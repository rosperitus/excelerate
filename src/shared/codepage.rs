//! Single-byte code pages, for the formats that predate Unicode.
//!
//! BIFF5 and CSV both hand over bytes and leave the page to be worked out:
//! BIFF5 names it in a `CODEPAGE` record, CSV names it nowhere at all. What is
//! here is the pages those files actually carry - Windows Latin and Cyrillic,
//! Mac Roman, and the DOS Cyrillic page a Russian export still comes in - and
//! nothing else. A page not listed reads as 1252, which is what the rest of
//! this crate does with bytes it cannot place.

/// Windows Latin 1, the default of every Windows Excel.
pub const WINDOWS_1252: u16 = 1252;
/// Windows Cyrillic.
pub const WINDOWS_1251: u16 = 1251;
/// Mac Roman, which every Excel 5 for the Mac wrote.
pub const MAC_ROMAN: u16 = 10000;
/// DOS Cyrillic.
pub const DOS_866: u16 = 866;
/// UTF-8, which a `CODEPAGE` record is allowed to name.
pub const UTF8: u16 = 65001;
/// UTF-16, which BIFF8 says instead of naming a byte page.
pub const UTF16: u16 = 1200;

/// Decodes `bytes` in `page`.
///
/// A byte with no character in its page becomes the replacement character
/// rather than nothing: a hole is easier to see than a silently shorter
/// string.
#[must_use]
pub fn decode(page: u16, bytes: &[u8]) -> String {
    if page == UTF8 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    bytes.iter().map(|&b| character(page, b)).collect()
}

/// One byte as a character.
#[must_use]
pub fn character(page: u16, byte: u8) -> char {
    if byte < 0x80 {
        return char::from(byte);
    }
    let table = match page {
        WINDOWS_1251 => &CYRILLIC_1251,
        MAC_ROMAN => &MAC,
        DOS_866 => &CYRILLIC_866,
        _ => &LATIN_1252,
    };
    table[usize::from(byte) - 0x80]
}

/// The high half of Windows 1252. Only `0x80..=0x9F` differs from Latin-1,
/// where 1252 puts printable characters and Latin-1 puts control codes.
const LATIN_1252: [char; 128] = [
    '\u{20AC}', '\u{81}', '\u{201A}', '\u{192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{2C6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8D}', '\u{17D}', '\u{8F}',
    '\u{90}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{2DC}', '\u{2122}', '\u{161}', '\u{203A}', '\u{153}', '\u{9D}', '\u{17E}', '\u{178}',
    '\u{A0}', '\u{A1}', '\u{A2}', '\u{A3}', '\u{A4}', '\u{A5}', '\u{A6}', '\u{A7}', '\u{A8}',
    '\u{A9}', '\u{AA}', '\u{AB}', '\u{AC}', '\u{AD}', '\u{AE}', '\u{AF}', '\u{B0}', '\u{B1}',
    '\u{B2}', '\u{B3}', '\u{B4}', '\u{B5}', '\u{B6}', '\u{B7}', '\u{B8}', '\u{B9}', '\u{BA}',
    '\u{BB}', '\u{BC}', '\u{BD}', '\u{BE}', '\u{BF}', '\u{C0}', '\u{C1}', '\u{C2}', '\u{C3}',
    '\u{C4}', '\u{C5}', '\u{C6}', '\u{C7}', '\u{C8}', '\u{C9}', '\u{CA}', '\u{CB}', '\u{CC}',
    '\u{CD}', '\u{CE}', '\u{CF}', '\u{D0}', '\u{D1}', '\u{D2}', '\u{D3}', '\u{D4}', '\u{D5}',
    '\u{D6}', '\u{D7}', '\u{D8}', '\u{D9}', '\u{DA}', '\u{DB}', '\u{DC}', '\u{DD}', '\u{DE}',
    '\u{DF}', '\u{E0}', '\u{E1}', '\u{E2}', '\u{E3}', '\u{E4}', '\u{E5}', '\u{E6}', '\u{E7}',
    '\u{E8}', '\u{E9}', '\u{EA}', '\u{EB}', '\u{EC}', '\u{ED}', '\u{EE}', '\u{EF}', '\u{F0}',
    '\u{F1}', '\u{F2}', '\u{F3}', '\u{F4}', '\u{F5}', '\u{F6}', '\u{F7}', '\u{F8}', '\u{F9}',
    '\u{FA}', '\u{FB}', '\u{FC}', '\u{FD}', '\u{FE}', '\u{FF}',
];

/// The high half of Windows 1251.
const CYRILLIC_1251: [char; 128] = [
    'Ђ', 'Ѓ', '‚', 'ѓ', '„', '…', '†', '‡', '€', '‰', 'Љ', '‹', 'Њ', 'Ќ', 'Ћ', 'Џ', 'ђ', '‘', '’',
    '“', '”', '•', '–', '—', '\u{98}', '™', 'љ', '›', 'њ', 'ќ', 'ћ', 'џ', '\u{A0}', 'Ў', 'ў', 'Ј',
    '¤', 'Ґ', '¦', '§', 'Ё', '©', 'Є', '«', '¬', '\u{AD}', '®', 'Ї', '°', '±', 'І', 'і', 'ґ', 'µ',
    '¶', '·', 'ё', '№', 'є', '»', 'ј', 'Ѕ', 'ѕ', 'ї', 'А', 'Б', 'В', 'Г', 'Д', 'Е', 'Ж', 'З', 'И',
    'Й', 'К', 'Л', 'М', 'Н', 'О', 'П', 'Р', 'С', 'Т', 'У', 'Ф', 'Х', 'Ц', 'Ч', 'Ш', 'Щ', 'Ъ', 'Ы',
    'Ь', 'Э', 'Ю', 'Я', 'а', 'б', 'в', 'г', 'д', 'е', 'ж', 'з', 'и', 'й', 'к', 'л', 'м', 'н', 'о',
    'п', 'р', 'с', 'т', 'у', 'ф', 'х', 'ц', 'ч', 'ш', 'щ', 'ъ', 'ы', 'ь', 'э', 'ю', 'я',
];

/// The high half of DOS 866, the page a Russian export still comes in.
const CYRILLIC_866: [char; 128] = [
    'А', 'Б', 'В', 'Г', 'Д', 'Е', 'Ж', 'З', 'И', 'Й', 'К', 'Л', 'М', 'Н', 'О', 'П', 'Р', 'С', 'Т',
    'У', 'Ф', 'Х', 'Ц', 'Ч', 'Ш', 'Щ', 'Ъ', 'Ы', 'Ь', 'Э', 'Ю', 'Я', 'а', 'б', 'в', 'г', 'д', 'е',
    'ж', 'з', 'и', 'й', 'к', 'л', 'м', 'н', 'о', 'п', '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕',
    '╣', '║', '╗', '╝', '╜', '╛', '┐', '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦',
    '╠', '═', '╬', '╧', '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐',
    '▀', 'р', 'с', 'т', 'у', 'ф', 'х', 'ц', 'ч', 'ш', 'щ', 'ъ', 'ы', 'ь', 'э', 'ю', 'я', 'Ё', 'ё',
    'Є', 'є', 'Ї', 'ї', 'Ў', 'ў', '°', '∙', '·', '√', '№', '¤', '■', '\u{A0}',
];

/// The high half of Mac Roman, which is what three of the four BIFF5 files in
/// the test corpus say they are written in.
const MAC: [char; 128] = [
    'Ä', 'Å', 'Ç', 'É', 'Ñ', 'Ö', 'Ü', 'á', 'à', 'â', 'ä', 'ã', 'å', 'ç', 'é', 'è', 'ê', 'ë', 'í',
    'ì', 'î', 'ï', 'ñ', 'ó', 'ò', 'ô', 'ö', 'õ', 'ú', 'ù', 'û', 'ü', '†', '°', '¢', '£', '§', '•',
    '¶', 'ß', '®', '©', '™', '´', '¨', '≠', 'Æ', 'Ø', '∞', '±', '≤', '≥', '¥', 'µ', '∂', '∑', '∏',
    'π', '∫', 'ª', 'º', 'Ω', 'æ', 'ø', '¿', '¡', '¬', '√', 'ƒ', '≈', '∆', '«', '»', '…', '\u{A0}',
    'À', 'Ã', 'Õ', 'Œ', 'œ', '–', '—', '“', '”', '‘', '’', '÷', '◊', 'ÿ', 'Ÿ', '⁄', '€', '‹', '›',
    'ﬁ', 'ﬂ', '‡', '·', '‚', '„', '‰', 'Â', 'Ê', 'Á', 'Ë', 'È', 'Í', 'Î', 'Ï', 'Ì', 'Ó', 'Ô',
    '\u{F8FF}', 'Ò', 'Ú', 'Û', 'Ù', 'ı', 'ˆ', '˜', '¯', '˘', '˙', '˚', '¸', '˝', '˛', 'ˇ',
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_page_reads_its_own_high_half() {
        // The same byte is a different letter in each page.
        assert_eq!(character(WINDOWS_1252, 0xC0), 'À');
        assert_eq!(character(WINDOWS_1251, 0xC0), 'А');
        assert_eq!(character(DOS_866, 0x80), 'А');
        assert_eq!(character(MAC_ROMAN, 0x80), 'Ä');
        // A page nobody listed falls back to 1252 rather than to nothing.
        assert_eq!(character(28_591, 0xC0), 'À');
        // ASCII is ASCII everywhere.
        assert_eq!(character(WINDOWS_1251, b'A'), 'A');
    }

    #[test]
    fn utf8_is_decoded_as_utf8() {
        assert_eq!(decode(UTF8, "Ёжик".as_bytes()), "Ёжик");
        // And the same bytes in a byte page are the mojibake they really are.
        assert_eq!(decode(WINDOWS_1251, &[0xC0, 0xC1]), "АБ");
    }
}
