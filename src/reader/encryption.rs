//! Workbooks saved with a password to open.
//!
//! Excel 2010 and later encrypt an xlsx, xlsm or xlsb the same way: the zip
//! package is encrypted whole and put in a compound file next to a stream
//! saying how (MS-OFFCRYPTO 2.3.4.10, "agile" encryption). Decrypting gives
//! back the zip, which the ordinary readers take from there.
//!
//! ponytail: agile only. The "standard" encryption of Excel 2007 and the RC4
//! of xls are refused with [`Error::Encrypted`]; there is no file of either
//! here to check a decryptor against.

use crate::error::{Error, Result};
use crate::model::protection::unbase64;
use aes::cipher::{BlockCipherDecrypt, KeyInit};
use sha2::Digest;

/// The password Excel uses for a workbook encrypted only to be opened
/// read-only, and tries before asking for one.
pub const DEFAULT_PASSWORD: &str = "VelvetSweatshop";

/// The spin count a file may ask for. Excel writes 100,000; a file asking
/// for billions would hold the reader for hours on a password nobody typed.
const MAX_SPIN_COUNT: u32 = 10_000_000;

/// Whether `bytes` is a compound file holding an encrypted package.
#[must_use]
pub fn is_encrypted(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0])
        && super::ole::Ole::new(bytes).is_ok_and(|ole| {
            ole.stream("EncryptionInfo").is_some() && ole.stream("EncryptedPackage").is_some()
        })
}

/// The zip package inside an encrypted workbook, opened with `password`.
///
/// # Errors
///
/// [`Error::WrongPassword`] when the password does not open it, and
/// [`Error::Encrypted`] when the encryption is not one this reads.
pub fn decrypt(bytes: &[u8], password: &str) -> Result<Vec<u8>> {
    let ole = super::ole::Ole::new(bytes).map_err(Error::Encrypted)?;
    let info = ole
        .stream("EncryptionInfo")
        .ok_or_else(|| Error::Encrypted("no EncryptionInfo stream".to_owned()))?;
    let package = ole
        .stream("EncryptedPackage")
        .ok_or_else(|| Error::Encrypted("no EncryptedPackage stream".to_owned()))?;
    // Version 4.4 with the agile flag; 2.2, 3.2 and 4.2 are the standard
    // encryption of Excel 2007.
    match info.get(..4) {
        Some([4, 0, 4, 0]) => {}
        _ => {
            return Err(Error::Encrypted(
                "only the agile encryption of Excel 2010 and later is read".to_owned(),
            ));
        }
    }
    let xml = std::str::from_utf8(info.get(8..).unwrap_or_default())
        .map_err(|_| Error::Encrypted("EncryptionInfo is not UTF-8".to_owned()))?;
    let agile = Agile::parse(xml)?;
    let key = agile.secret_key(password)?;
    agile.package(&key, &package)
}

/// A hash the descriptor may name.
#[derive(Debug, Clone, Copy)]
enum Hash {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

impl Hash {
    fn parse(name: &str) -> Result<Self> {
        match name {
            "SHA1" | "SHA-1" => Ok(Self::Sha1),
            "SHA256" => Ok(Self::Sha256),
            "SHA384" => Ok(Self::Sha384),
            "SHA512" => Ok(Self::Sha512),
            other => Err(Error::Encrypted(format!("hash {other} is not supported"))),
        }
    }

    /// The hash of the parts one after another.
    fn of(self, parts: &[&[u8]]) -> Vec<u8> {
        fn run<D: Digest>(parts: &[&[u8]]) -> Vec<u8> {
            let mut d = D::new();
            for part in parts {
                d.update(part);
            }
            d.finalize().to_vec()
        }
        match self {
            Self::Sha1 => run::<sha1::Sha1>(parts),
            Self::Sha256 => run::<sha2::Sha256>(parts),
            Self::Sha384 => run::<sha2::Sha384>(parts),
            Self::Sha512 => run::<sha2::Sha512>(parts),
        }
    }
}

/// One `keyData` or password `encryptedKey` of the descriptor: how a key is
/// made and what it encrypts.
#[derive(Debug, Default)]
struct Params {
    salt: Vec<u8>,
    hash: Option<Hash>,
    key_bits: usize,
    block_size: usize,
}

/// The descriptor of agile encryption.
#[derive(Debug, Default)]
struct Agile {
    /// What encrypts the package.
    data: Params,
    /// What encrypts the key, derived from the password.
    password: Params,
    spin_count: u32,
    verifier_input: Vec<u8>,
    verifier_hash: Vec<u8>,
    key_value: Vec<u8>,
}

/// Block keys of MS-OFFCRYPTO 2.3.4.13, one per thing the password unlocks.
const VERIFIER_INPUT_BLOCK: [u8; 8] = [0xFE, 0xA7, 0xD2, 0x76, 0x3B, 0x4B, 0x9E, 0x79];
const VERIFIER_HASH_BLOCK: [u8; 8] = [0xD7, 0xAA, 0x0F, 0x6D, 0x30, 0x61, 0x34, 0x4E];
const KEY_VALUE_BLOCK: [u8; 8] = [0x14, 0x6E, 0x0B, 0xE7, 0xAB, 0xAC, 0xD0, 0xD6];

/// The bytes a package is cut into, each encrypted with its own IV.
const SEGMENT: usize = 4096;

impl Agile {
    fn parse(xml: &str) -> Result<Self> {
        use quick_xml::events::Event;
        let bad = |what: &str| Error::Encrypted(format!("EncryptionInfo: {what}"));
        let mut reader = quick_xml::Reader::from_str(xml);
        let mut out = Self::default();
        let (mut saw_data, mut saw_password) = (false, false);
        loop {
            match reader.read_event() {
                Ok(Event::Start(e) | Event::Empty(e)) => {
                    let name = e.local_name();
                    let target = match name.as_ref() {
                        "keyData" => {
                            saw_data = true;
                            &mut out.data
                        }
                        // A certificate's key sits beside the password's.
                        "encryptedKey"
                            if e.name().as_ref().starts_with("p:")
                                || e.attributes()
                                    .flatten()
                                    .any(|a| a.key.local_name().as_ref() == "spinCount") =>
                        {
                            saw_password = true;
                            &mut out.password
                        }
                        _ => continue,
                    };
                    for a in e.attributes().flatten() {
                        let value = a
                            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                            .map_err(|_| bad("attribute"))?;
                        let base64 = || unbase64(&value).ok_or_else(|| bad("base64"));
                        match a.key.local_name().as_ref() {
                            "saltValue" => target.salt = base64()?,
                            "hashAlgorithm" => target.hash = Some(Hash::parse(&value)?),
                            "keyBits" => {
                                target.key_bits = value.parse().map_err(|_| bad("keyBits"))?;
                            }
                            "blockSize" => {
                                target.block_size = value.parse().map_err(|_| bad("blockSize"))?;
                            }
                            "cipherAlgorithm" if &*value != "AES" => {
                                return Err(Error::Encrypted(format!(
                                    "cipher {value} is not supported"
                                )));
                            }
                            "cipherChaining" if &*value != "ChainingModeCBC" => {
                                return Err(Error::Encrypted(format!(
                                    "chaining {value} is not supported"
                                )));
                            }
                            "spinCount" => {
                                out.spin_count = value.parse().map_err(|_| bad("spinCount"))?;
                            }
                            "encryptedVerifierHashInput" => out.verifier_input = base64()?,
                            "encryptedVerifierHashValue" => out.verifier_hash = base64()?,
                            "encryptedKeyValue" => out.key_value = base64()?,
                            _ => {}
                        }
                    }
                }
                Ok(Event::Eof) => break,
                Err(_) => return Err(bad("not XML")),
                _ => {}
            }
        }
        if !saw_data || !saw_password {
            return Err(Error::Encrypted(
                "no password key; a workbook encrypted to a certificate is not read".to_owned(),
            ));
        }
        if out.spin_count > MAX_SPIN_COUNT {
            return Err(bad("spin count out of range"));
        }
        Ok(out)
    }

    /// The key that encrypts the package, unlocked with the password.
    fn secret_key(&self, password: &str) -> Result<Vec<u8>> {
        let p = &self.password;
        let hash = p
            .hash
            .ok_or_else(|| Error::Encrypted("no hash algorithm".to_owned()))?;
        let utf16: Vec<u8> = password.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut h = hash.of(&[&p.salt, &utf16]);
        for i in 0..self.spin_count {
            h = hash.of(&[&i.to_le_bytes(), &h]);
        }
        let key_for = |block: &[u8]| fit(hash.of(&[&h, block]), p.key_bits / 8, 0x36);
        let decrypt = |block: &[u8], data: &[u8]| cbc_decrypt(&key_for(block), &p.salt, data);

        let input = decrypt(&VERIFIER_INPUT_BLOCK, &self.verifier_input)?;
        let expected = decrypt(&VERIFIER_HASH_BLOCK, &self.verifier_hash)?;
        let input = input.get(..p.salt.len()).unwrap_or(&input);
        let got = hash.of(&[input]);
        if expected.get(..got.len()) != Some(got.as_slice()) {
            return Err(Error::WrongPassword);
        }
        let mut key = decrypt(&KEY_VALUE_BLOCK, &self.key_value)?;
        key.truncate(self.data.key_bits / 8);
        Ok(key)
    }

    /// The zip package: a length, then segments decrypted each with an IV of
    /// its own number.
    fn package(&self, key: &[u8], stream: &[u8]) -> Result<Vec<u8>> {
        let d = &self.data;
        let hash = d
            .hash
            .ok_or_else(|| Error::Encrypted("no hash algorithm".to_owned()))?;
        let (size, body) = stream
            .split_first_chunk::<8>()
            .ok_or_else(|| Error::Encrypted("EncryptedPackage too short".to_owned()))?;
        let mut out = Vec::with_capacity(body.len());
        for (i, segment) in body.chunks(SEGMENT).enumerate() {
            let index = u32::try_from(i)
                .map_err(|_| Error::Encrypted("EncryptedPackage too long".to_owned()))?;
            let iv = fit(
                hash.of(&[&d.salt, &index.to_le_bytes()]),
                d.block_size,
                0x36,
            );
            out.extend(cbc_decrypt(key, &iv, segment)?);
        }
        // The size is the file's word; it never makes the output longer.
        let size = usize::try_from(u64::from_le_bytes(*size)).unwrap_or(usize::MAX);
        out.truncate(size);
        Ok(out)
    }
}

/// A hash cut or padded to `len` bytes, as MS-OFFCRYPTO 2.3.4.12 derives a
/// key or an IV.
fn fit(mut bytes: Vec<u8>, len: usize, pad: u8) -> Vec<u8> {
    bytes.resize(len, pad);
    bytes
}

/// AES in CBC mode, without padding: the format pads to the block itself.
fn cbc_decrypt(key: &[u8], iv: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    fn run<C: BlockCipherDecrypt<BlockSize = aes::cipher::consts::U16> + KeyInit>(
        key: &[u8],
        iv: &[u8],
        data: &[u8],
    ) -> Result<Vec<u8>> {
        let cipher =
            C::new_from_slice(key).map_err(|_| Error::Encrypted("key length".to_owned()))?;
        let mut previous: [u8; 16] = iv
            .get(..16)
            .and_then(|iv| iv.try_into().ok())
            .ok_or_else(|| Error::Encrypted("IV shorter than a block".to_owned()))?;
        let mut out = Vec::with_capacity(data.len());
        for chunk in data.chunks(16) {
            let Ok(block) = <[u8; 16]>::try_from(chunk) else {
                return Err(Error::Encrypted("data is not whole blocks".to_owned()));
            };
            let mut plain = block.into();
            cipher.decrypt_block(&mut plain);
            out.extend(plain.iter().zip(previous).map(|(p, v)| p ^ v));
            previous = block;
        }
        Ok(out)
    }
    match key.len() {
        16 => run::<aes::Aes128>(key, iv, data),
        24 => run::<aes::Aes192>(key, iv, data),
        32 => run::<aes::Aes256>(key, iv, data),
        n => Err(Error::Encrypted(format!("an AES key of {n} bytes"))),
    }
}
