//! What the formula engine computes, checked against Excel's own answers.
//!
//! Every expectation here is what Excel shows for that formula; where
//! another implementation disagrees with Excel the deviation is called out in the
//! comment beside it.

// A failing assertion is this suite's output; panicking is the report.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use excelerate::coordinate::CellRef;
use excelerate::error::CellError;
use excelerate::formula::eval::{Engine, Origin};
use excelerate::formula::value::Value;
use excelerate::model::{CellValue, DefinedName, Spreadsheet, Worksheet};

fn at(a: &str) -> CellRef {
    CellRef::parse(a).expect("test reference is valid")
}

/// A workbook holding the little table every case below refers to.
///
/// ```text
///      A      B        C          D
/// 1    1      "x"      #DIV/0!    10
/// 2    2      TRUE                20
/// 3    3      (blank)             30
/// ```
fn book() -> Spreadsheet {
    let mut wb = Spreadsheet::empty();
    let mut ws = Worksheet::new("Data").unwrap();
    for (row, n) in [(1, 1.0), (2, 2.0), (3, 3.0)] {
        ws.set(at(&format!("A{row}")), n);
        ws.set(at(&format!("D{row}")), n * 10.0);
    }
    ws.set(at("B1"), "x");
    ws.set(at("B2"), true);
    ws.entry(at("C1")).value = CellValue::Error(CellError::Div0);
    wb.add_sheet(ws).unwrap();

    let mut other = Worksheet::new("Другой лист").unwrap();
    other.set(at("A1"), 42.0);
    wb.add_sheet(other).unwrap();

    wb.defined_names.push(DefinedName {
        name: "Ставка".into(),
        sheet: None,
        formula: "0.15".into(),
        hidden: false,
    });
    wb.defined_names.push(DefinedName {
        name: "Числа".into(),
        sheet: None,
        formula: "Data!$A$1:$A$3".into(),
        hidden: false,
    });
    // The same name means one thing on one sheet and another on the next.
    wb.defined_names.push(DefinedName {
        name: "Свой".into(),
        sheet: Some(0),
        formula: "111".into(),
        hidden: false,
    });
    wb.defined_names.push(DefinedName {
        name: "Свой".into(),
        sheet: Some(1),
        formula: "222".into(),
        hidden: false,
    });
    // A name that stands for itself.
    wb.defined_names.push(DefinedName {
        name: "Петля".into(),
        sheet: None,
        formula: "Петля+1".into(),
        hidden: false,
    });
    wb
}

/// Renders a value the way the cases below spell their expectations.
fn show(v: &Value) -> String {
    match v {
        Value::Blank => "(blank)".to_owned(),
        Value::Number(n) => format!("{n}"),
        Value::Text(t) => format!("\"{t}\""),
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_owned(),
        Value::Error(e) => e.as_str().to_owned(),
        Value::Lambda(_) => "(lambda)".to_owned(),
        Value::Array(rows) => {
            let rows: Vec<String> = rows
                .iter()
                .map(|r| r.iter().map(show).collect::<Vec<_>>().join(" "))
                .collect();
            format!("{{{}}}", rows.join("; "))
        }
    }
}

/// Checks a table of formulas against the values Excel gives for them.
#[track_caller]
fn check(cases: &[(&str, &str)]) {
    let wb = book();
    let mut engine = Engine::new(&wb);
    let origin = Origin::new(0, at("Z100"));
    for (formula, expected) in cases {
        let got = show(&engine.eval(origin, formula));
        assert_eq!(got, *expected, "{formula}");
    }
}

#[test]
fn arithmetic_and_operators() {
    check(&[
        ("1+2", "3"),
        ("2*3+4", "10"),
        ("(2+3)*4", "20"),
        ("10/4", "2.5"),
        ("1/0", "#DIV/0!"),
        // A leading minus binds tighter than the power, so this is 4.
        ("-2^2", "4"),
        // Excel's power is left-associative, unlike the mathematical
        // convention: this is 64, not 512.
        ("2^3^2", "64"),
        ("50%", "0.5"),
        ("2%*100", "2"),
        // A leading plus is not an operator: text and logicals pass through,
        // as `=+Sheet!A1` in an old workbook expects. A minus still converts.
        ("+\"abc\"", "\"abc\""),
        ("+TRUE", "TRUE"),
        ("-\"abc\"", "#VALUE!"),
        ("\"a\"&1", "\"a1\""),
        ("\"a\"&TRUE", "\"aTRUE\""),
        ("1&\"\"", "\"1\""),
        ("(-8)^(1/3)", "#NUM!"),
    ]);
}

#[test]
fn comparison_follows_excel_type_order() {
    check(&[
        ("1=1", "TRUE"),
        ("1<>2", "TRUE"),
        ("\"a\"=\"A\"", "TRUE"),
        ("EXACT(\"a\",\"A\")", "FALSE"),
        ("2>\"1\"", "FALSE"),
        // Every number sorts before every piece of text, and text before the
        // booleans. Comparing them as plain strings would answer TRUE.
        ("\"z\">TRUE", "FALSE"),
        ("9E+30<\"a\"", "TRUE"),
        ("B3=0", "TRUE"),
        ("B3=\"\"", "TRUE"),
    ]);
}

#[test]
fn references_are_read_and_errors_travel() {
    check(&[
        ("A1", "1"),
        ("A1+A2", "3"),
        ("B1", "\"x\""),
        // A whole formula is never empty: an empty cell read out is 0.
        ("B3", "0"),
        ("B3+1", "1"),
        ("C1", "#DIV/0!"),
        ("C1+1", "#DIV/0!"),
        ("SUM(C1:C1)", "#DIV/0!"),
        ("'Другой лист'!A1", "42"),
        ("Data!A2", "2"),
        ("NoSuchSheet!A1", "#REF!"),
        // Two references separated by a space are the cells they share.
        ("A1:A3 A2:D2", "2"),
        ("A1:A1 B1:B1", "#NULL!"),
        ("SUM((A1,A3))", "4"),
    ]);
}

#[test]
fn aggregates_treat_literals_and_cells_differently() {
    check(&[
        ("SUM(A1:A3)", "6"),
        ("SUM(A1:A3,10)", "16"),
        // Text and booleans read out of cells are skipped ...
        ("SUM(B1:B3)", "0"),
        // ... but the same values written into the formula are converted.
        ("SUM(TRUE,\"1\")", "2"),
        ("PRODUCT(A1:A3)", "6"),
        ("AVERAGE(A1:A3)", "2"),
        ("AVERAGE(B1:B3)", "#DIV/0!"),
        ("COUNT(A1:B3)", "3"),
        ("COUNTA(A1:B3)", "5"),
        ("COUNTBLANK(A1:B3)", "1"),
        ("MAX(A1:A3)", "3"),
        ("MIN(A1:A3)", "1"),
        ("MEDIAN(A1:A3)", "2"),
        // `MEDIANIF` mirrors `AVERAGEIF`: same shape, median instead of mean.
        ("MEDIANIF(A1:A3,\">1\")", "2.5"),
        ("MEDIANIF(A1:A3,\">9\")", "#NUM!"),
        ("MAX(B1:B3)", "0"),
    ]);
}

#[test]
fn logic_computes_only_the_branch_it_takes() {
    check(&[
        ("IF(A1=1,\"y\",\"n\")", "\"y\""),
        // The untaken branch would divide by zero if it were computed.
        ("IF(A1=0,1/0,5)", "5"),
        ("IF(A1=1,1/0,5)", "#DIV/0!"),
        ("IF(FALSE,1)", "FALSE"),
        // An omitted branch is 0, not an empty cell.
        ("IF(A1,,7)", "0"),
        ("IFERROR(1/0,\"caught\")", "\"caught\""),
        ("IFERROR(A1,\"caught\")", "1"),
        ("IFNA(1/0,\"caught\")", "#DIV/0!"),
        ("AND(TRUE,A1=1)", "TRUE"),
        ("AND(TRUE,FALSE)", "FALSE"),
        ("OR(FALSE,A1=9)", "FALSE"),
        ("XOR(TRUE,TRUE)", "FALSE"),
        ("NOT(FALSE)", "TRUE"),
        // Text among the arguments is not a logical value; Excel skips it.
        ("AND(TRUE,B1)", "TRUE"),
    ]);
}

#[test]
fn text_functions_count_characters_not_bytes() {
    check(&[
        ("LEN(\"Ёж\")", "2"),
        ("LEFT(\"абв\",2)", "\"аб\""),
        ("RIGHT(\"абв\",2)", "\"бв\""),
        ("MID(\"abcdef\",2,3)", "\"bcd\""),
        ("TRIM(\"  a   b  \")", "\"a b\""),
        ("UPPER(\"ёж\")", "\"ЁЖ\""),
        ("LOWER(\"ЁЖ\")", "\"ёж\""),
        ("CONCATENATE(\"a\",1,TRUE)", "\"a1TRUE\""),
        ("REPT(\"ab\",3)", "\"ababab\""),
        ("FIND(\"b\",\"abc\")", "2"),
        ("FIND(\"B\",\"abc\")", "#VALUE!"),
        ("SEARCH(\"B\",\"abc\")", "2"),
        ("SUBSTITUTE(\"aaa\",\"a\",\"b\")", "\"bbb\""),
        ("SUBSTITUTE(\"aaa\",\"a\",\"b\",2)", "\"aba\""),
        ("VALUE(\"1.5\")", "1.5"),
        ("VALUE(\"x\")", "#VALUE!"),
        ("TEXT(0.5,\"0.00\")", "\"0.50\""),
        ("TEXT(45658,\"dd.mm.yyyy\")", "\"01.01.2025\""),
    ]);
}

#[test]
fn information_functions() {
    check(&[
        ("ISBLANK(B3)", "TRUE"),
        ("ISBLANK(A1)", "FALSE"),
        ("ISNUMBER(A1)", "TRUE"),
        ("ISTEXT(B1)", "TRUE"),
        ("ISLOGICAL(B2)", "TRUE"),
        ("ISERROR(C1)", "TRUE"),
        ("ISERR(C1)", "TRUE"),
        ("ISERR(NA())", "FALSE"),
        ("ISNA(NA())", "TRUE"),
        ("N(A1)", "1"),
        ("N(B1)", "0"),
        ("TYPE(B1)", "2"),
        ("TYPE(A1:A3)", "64"),
    ]);
}

#[test]
fn lookup_and_reference() {
    check(&[
        ("ROW(A5)", "5"),
        ("COLUMN(C1)", "3"),
        // With no argument they answer for the cell the formula sits in.
        ("ROW()", "100"),
        ("COLUMN()", "26"),
        ("ROWS(A1:A3)", "3"),
        ("COLUMNS(A1:C1)", "3"),
        ("CHOOSE(2,\"a\",\"b\")", "\"b\""),
        ("VLOOKUP(2,A1:D3,4,FALSE)", "20"),
        ("VLOOKUP(2.5,A1:D3,4)", "20"),
        ("VLOOKUP(9,A1:D3,4,FALSE)", "#N/A"),
        ("HLOOKUP(1,A1:D3,3,FALSE)", "3"),
        ("MATCH(2,A1:A3,0)", "2"),
        ("MATCH(2.5,A1:A3)", "2"),
        ("MATCH(9,A1:A3,0)", "#N/A"),
        ("INDEX(A1:D3,2,4)", "20"),
        ("INDEX(A1:D3,5,1)", "#REF!"),
        // Over a range they are every row or column of it, which is what makes
        // `A1:D1+COLUMN(A1:D1)` a tie-breaking array rather than one number.
        ("ROW(A1:A3)", "{1; 2; 3}"),
        ("COLUMN(A1:D1)", "{1 2 3 4}"),
        // One row and no column asked for: the number picks the column.
        ("INDEX(A1:D1,1)", "1"),
        ("INDEX(A1:D1,4)", "10"),
        ("INDEX(A1:D1,5)", "#REF!"),
        // A column still counts down, and an explicit 0 still means "all".
        ("INDEX(A1:A3,2)", "2"),
        ("INDEX(A1:D1,0)", "{1 \"x\" #DIV/0! 10}"),
        ("INDEX(A1:A3,MATCH(LARGE(A1:A3,1),A1:A3,0))", "3"),
    ]);
}

#[test]
fn text_functions_added_with_the_maths() {
    check(&[
        ("PROPER(\"hello world\")", "\"Hello World\""),
        // A title-casing routine that keeps a
        // word together across an apostrophe and answers "O'neil". Excel breaks
        // at any non-letter -- its own documentation gives PROPER("2-cent's
        // worth") as "2-Cent'S Worth".
        ("PROPER(\"o'neil 2nd\")", "\"O'Neil 2Nd\""),
        ("CLEAN(\"a\"&CHAR(9)&\"b\")", "\"ab\""),
        ("T(\"x\")", "\"x\""),
        ("T(1)", "\"\""),
        ("CHAR(65)", "\"A\""),
        ("CODE(\"A\")", "65"),
        ("UNICHAR(1071)", "\"Я\""),
        ("UNICODE(\"Я\")", "1071"),
        ("REPLACE(\"abcdef\",2,3,\"X\")", "\"aXef\""),
        ("TEXTJOIN(\"-\",TRUE,\"a\",\"\",\"b\")", "\"a-b\""),
        ("TEXTBEFORE(\"a-b-c\",\"-\",2)", "\"a-b\""),
        ("TEXTAFTER(\"a-b-c\",\"-\",-1)", "\"c\""),
        ("FIXED(1234.567,1)", "\"1,234.6\""),
        // Excel rounds a half away from zero; Rust's formatting rounds to even.
        ("FIXED(0.5,0)", "\"1\""),
        ("FIXED(-1234.5,0)", "\"-1,235\""),
        ("NUMBERVALUE(\"1,5\",\",\",\" \")", "1.5"),
        // An implementation refusing text with no decimal separator reads "50%"
        // and even "2" come back #VALUE!. Excel documents NUMBERVALUE("9%%")
        // as 0.0009.
        ("NUMBERVALUE(\"50%\")", "0.5"),
        ("NUMBERVALUE(\"2\")", "2"),
        // The decimal separator repeated, or a group separator after it, is
        // not a number -- which is also how the two being the same is caught.
        ("NUMBERVALUE(\"1,5\",\",\")", "#VALUE!"),
    ]);
}

#[test]
fn finance_keeps_excels_sign_convention() {
    check(&[
        // Money paid out is negative: a loan's payment leaves your pocket.
        ("PMT(0,10,1000)", "-100"),
        ("FV(0,10,-100)", "1000"),
        ("PV(0,10,-100)", "1000"),
        ("NPER(0,-100,1000)", "10"),
        // A payment of nothing never repays anything.
        ("NPER(0.05,0,1000)", "#NUM!"),
        // The period has to be one of the ones there are.
        ("IPMT(0.05,0,10,1000)", "#NUM!"),
        ("IPMT(0.05,11,10,1000)", "#NUM!"),
        ("SYD(10000,1000,5,6)", "#NUM!"),
        ("SLN(10000,1000,0)", "#DIV/0!"),
        // A left-out argument in the middle is a zero, not a missing one.
        ("PMT(0,10,1000,,0)", "-100"),
        // The interest and the principal of a period add up to the payment.
        (
            "IPMT(0.05,3,10,1000)+PPMT(0.05,3,10,1000)-PMT(0.05,10,1000)",
            "0",
        ),
        // Cash flows with no sign change have no rate that zeroes them.
        ("IRR({1,2,3})", "#VALUE!"),
        ("MIRR({1,2,3},0.1,0.1)", "#DIV/0!"),
        ("EFFECT(0.1,0)", "#NUM!"),
        ("DOLLARDE(1.02,0)", "#DIV/0!"),
        ("DOLLARFR(1.125,16)", "1.02"),
    ]);
}

#[test]
fn cases_a_third_engine_settled() {
    // Found by running excelize's own test expectations through this engine,
    // then confirming each against both other implementations.
    check(&[
        // Only a positive number with a negative step is refused. A negative
        // number rounds to a positive step perfectly well.
        ("FLOOR(-2.05,2)", "-4"),
        ("CEILING(-2.05,2)", "-2"),
        ("FLOOR(-2.5,-2)", "-2"),
        ("CEILING(-2.5,-2)", "-4"),
        ("FLOOR(2.5,-2)", "#NUM!"),
        ("CEILING(2.5,-2)", "#NUM!"),
        // Both branches of IF are optional; with neither, it answers the
        // condition itself.
        ("IF(1=1)", "TRUE"),
        ("IF(1<>1)", "FALSE"),
        ("IF(1=1,5)", "5"),
        ("IF(1<>1,5)", "FALSE"),
        // SEARCH takes wildcards, FIND does not.
        ("SEARCH(\"?e\",\"Sales\")", "3"),
        ("SEARCH(\"s*s\",\"Sales\")", "1"),
        ("SEARCH(\"e*\",\"Sales\")", "4"),
        ("SEARCH(\"z*\",\"Sales\")", "#VALUE!"),
        // A tilde makes the next character itself rather than a wildcard.
        ("SEARCH(\"~?\",\"a?b\")", "2"),
        ("FIND(\"?e\",\"Sales\")", "#VALUE!"),
    ]);
}

#[test]
fn text_that_spells_a_date_is_read_as_one() {
    check(&[
        // The American order, which is the one Excel reads. Following the
        // machine's locale instead makes
        // this the third of March -- serial 30746 -- on a European box.
        ("DATEVALUE(\"05/03/1984\")", "30805"),
        ("DATEVALUE(\"12/25/2012\")", "41268"),
        ("DAY(\"01/03/2019 12:14:16\")", "3"),
        // A month past twelve is no month at all, whatever the day.
        ("DATEVALUE(\"31/12/2015\")", "#VALUE!"),
        // Four digits first means the year leads.
        ("DATEVALUE(\"2015-05-31\")", "42155"),
        // A two-digit year turns at thirty.
        ("DATEVALUE(\"1/1/29\")", "47119"),
        ("DATEVALUE(\"1/1/30\")", "10959"),
        // A month name settles which number is which, in any case and with
        // the ordinal suffix Excel allows.
        ("DATEVALUE(\"31-MAY-2015\")", "42155"),
        ("DATEVALUE(\"1st May 2015\")", "42125"),
        ("DATEVALUE(\"25-Jan-20\")", "43855"),
        // A time is a fraction, and text holding both gives each function its
        // own part of it.
        ("HOUR(\"1:00 PM\")", "13"),
        ("MINUTE(\"12/09/2015 08:55\")", "55"),
        ("TIMEVALUE(\"12:00 AM\")", "0"),
        ("TIMEVALUE(\"12:00 PM\")", "0.5"),
        // DATEVALUE refuses a bare time; TIMEVALUE reads the same string.
        ("DATEVALUE(\"13:35:55\")", "#VALUE!"),
        ("DATEVALUE(\"nonsense\")", "#VALUE!"),
        // A dot is not a date separator in Excel's own locale.
        ("DATEVALUE(\"1.1.2020\")", "#VALUE!"),
        // Text that merely looks like a date stays text to a function that
        // asked for a number.
        ("SUM(\"2015-05-31\")", "#VALUE!"),
        ("DAYS(\"2015-05-31\",\"2015-05-01\")", "30"),
    ]);
}

#[test]
fn more_cases_the_third_engine_settled() {
    check(&[
        // Text spelling a number is that serial already, not a date read out
        // of its digits: DAY("35") is the fourth of February 1900.
        ("DAY(\"35\")", "4"),
        ("YEAR(\"15\")", "1900"),
        ("MINUTE(\"0.04\")", "57"),
        // MATCH takes wildcards where the match is exact, and not where the
        // list is meant to be sorted.
        ("MATCH(\"b*\",{\"aa\",\"bb\",\"cc\"},0)", "2"),
        ("MATCH(\"?c\",{\"aa\",\"bb\",\"cc\"},0)", "3"),
        ("MATCH(\"zz*\",{\"aa\",\"bb\",\"cc\"},0)", "#N/A"),
        // VALUE takes any shape Excel calls a number, date or time.
        ("VALUE(\"5,000\")", "5000"),
        ("VALUE(\"$1,000\")", "1000"),
        ("VALUE(\"50%\")", "0.5"),
        ("VALUE(\"12:00:00\")", "0.5"),
        ("VALUE(\"2015-05-31\")", "42155"),
        ("VALUE(\"abc\")", "#VALUE!"),
        // Characters, not bytes, wherever a count is taken.
        ("LEN(\"テキスト\")", "4"),
        ("LEFT(\"オリジナルテキスト\",2)", "\"オリ\""),
        ("RIGHT(\"オリジナルテキスト\",4)", "\"テキスト\""),
        ("MID(\"你好World\",5,1)", "\"r\""),
        ("SEARCH(\"?l\",\"你好world\")", "5"),
    ]);
}

#[test]
fn a_colon_between_areas_spans_them() {
    check(&[
        // A1:A2 and A3 together are A1:A3.
        ("SUM(A1:A2:A3)", "6"),
        ("SUM(A3:A1:A2)", "6"),
        ("SUM(D1:D2:D3)", "60"),
        // The rectangle is the whole of what it covers, so an error inside it
        // travels out: A1:A2:C1 spans A1:C2, and C1 holds #DIV/0!.
        ("SUM(A1:A2:C1)", "#DIV/0!"),
        // Text and booleans among the cells are skipped, not converted --
        // what a colon builds is a reference, the same as A1:D2 is.
        ("SUM(A1:A2:B2)", "3"),
        // The chain keeps going, and ROWS and COLUMNS measure the reference
        // rather than the values behind it.
        ("COLUMNS(A1:B2:D5)", "4"),
        ("ROWS(A1:B2:D5)", "5"),
        ("COLUMNS(A1:A2:D1:B3:C1)", "4"),
        // A whole row or column stays its full size, with or without a sheet.
        ("COLUMNS(1:1)", "16384"),
        ("COLUMNS(Data!1:1)", "16384"),
        ("ROWS(A:A)", "1048576"),
        ("ROWS(Data!A:A)", "1048576"),
        ("SUM(A:A)", "6"),
        ("SUM(Data!A:A)", "6"),
        // A space is still the intersection, and stays the narrower answer.
        ("SUM(A1:A2 A2:A3)", "2"),
    ]);
}

#[test]
fn statistics_other_engines_leave_out() {
    // Every expectation here is excelize's, which is the only other engine
    // that knows these names; the ones does know are fixtures.
    check(&[
        ("SKEW.P(1,2,3,4,10)", "1.1384199576606164"),
        // The exclusive quantiles leave room at both ends, so a quarter of the
        // way along four values is 1.25 rather than 1.75.
        ("PERCENTILE.EXC({1,2,3,4},0.25)", "1.25"),
        ("QUARTILE.EXC({1,2,3,4},1)", "1.25"),
        ("PERCENTRANK.EXC({1,2,3,4},3)", "0.6"),
        // k has to stay strictly inside the range the exclusive form allows.
        ("PERCENTILE.EXC({1,2,3,4},0)", "#NUM!"),
        ("PERCENTILE.EXC({1,2,3,4},0.1)", "#NUM!"),
        ("COVARIANCE.S({1,2,3},{2,4,7})", "2.5"),
        ("PHI(0.5)", "0.3520653267642995"),
        // Rounding this gives a different answer where Excel and excelize
        // truncate: two thirds
        // to three digits is 0.666, not 0.667.
        ("PERCENTRANK({1,2,3,4},3)", "0.666"),
    ]);
}

#[test]
fn the_inverse_normal_is_exact_where_both_engines_approximate() {
    // excelize carries a nine-digit approximation for
    // the inverse normal; bisecting the exact cdf reaches twelve. The
    // expectations are the true values, confirmed against Python's
    // statistics.NormalDist, which agrees with them to the last digit shown.
    check(&[
        ("ROUND(NORMSINV(0.9),11)", "1.28155156554"),
        ("ROUND(NORMSINV(0.975),11)", "1.95996398454"),
        // Zero, up to the sign a bisection leaves on it -- which all three
        // engines print as -0 for a rounded negative.
        ("ROUND(ABS(NORMSINV(0.5)),11)", "0"),
        ("ROUND(NORMINV(0.9,40,1.5),9)", "41.922327348"),
        ("ROUND(CONFIDENCE(0.05,2.5,50),9)", "0.692951912"),
        // Both ends are outside what a probability may be.
        ("NORMSINV(0)", "#NUM!"),
        ("NORMSINV(1)", "#NUM!"),
    ]);
}

#[test]
fn the_a_forms_count_text_as_zero() {
    check(&[
        // AVERAGE skips the text and the boolean in B1 and B2; AVERAGEA counts
        // them, as zero and one.
        ("AVERAGE(A1:B2)", "1.5"),
        ("AVERAGEA(A1:B2)", "1"),
        ("MAXA(A1:B2)", "2"),
        ("MINA(A1:B2)", "0"),
        // Text written as the argument itself is no cell: it has to be a
        // number, and Excel's own cache has `MINA(10,55,"текст",89)` as
        // #VALUE!. Numeric text converts.
        ("MINA(1,\"x\")", "#VALUE!"),
        ("VARA(1,2,\"x\",TRUE)", "#VALUE!"),
        ("AVERAGEA(10,\"20\")", "15"),
        ("VARPA(1,2,\"x\",TRUE)", "#VALUE!"),
        ("STDEVA(1,2,\"x\",TRUE)", "#VALUE!"),
        // Text read out of cells counts as zero: B1 holds text, B2 TRUE.
        ("VARPA(1,2,B1,B2)", "0.5"),
        // Nothing matched is zero here, not an error.
        ("MAXIFS(D1:D3,A1:A3,\">1\")", "30"),
        ("MINIFS(D1:D3,A1:A3,\">1\")", "20"),
        ("MAXIFS(D1:D3,A1:A3,\">9\")", "0"),
    ]);
}

#[test]
fn tests_of_a_hypothesis() {
    // Confirmed against excelize; has only two of these and agrees
    // with both, as the fixtures record.
    check(&[
        ("CHITEST({10,20},{15,15})", "0.06788915486173791"),
        ("FTEST({1,2,3,4},{2,4,6,8})", "0.28475697986529425"),
        // Paired, equal spread, and Welch: three different answers.
        ("TTEST({1,2,3,4},{2,4,6,8},2,1)", "0.030466291662170297"),
        ("TTEST({1,2,3,4},{2,4,6,8},2,2)", "0.13397459621554964"),
        ("TTEST({1,2,3,4},{2,4,6,8},2,3)", "0.15158050484529717"),
        ("ZTEST({1,2,3,4},2)", "0.2192890130405024"),
        ("TTEST({1,2},{3,4},3,1)", "#NUM!"),
    ]);
}

#[test]
fn frequency_counts_into_bins_and_the_tail() {
    check(&[
        // Bins {2,4} make three buckets: up to 2, up to 4, and the rest.
        ("INDEX(FREQUENCY({1,2,3,4,5},{2,4}),1)", "2"),
        ("INDEX(FREQUENCY({1,2,3,4,5},{2,4}),2)", "2"),
        ("INDEX(FREQUENCY({1,2,3,4,5},{2,4}),3)", "1"),
        ("ROWS(FREQUENCY({1,2,3,4,5},{2,4}))", "3"),
        // Every value tied for most common, in the order they first appear.
        ("INDEX(MODE.MULT({1,2,2,3,3}),1)", "2"),
        ("INDEX(MODE.MULT({1,2,2,3,3}),2)", "3"),
        ("MODE.MULT({1,2,3})", "#N/A"),
        // Ties share the places they take up.
        ("RANK.AVG(2,{1,2,2,3})", "2.5"),
        ("RANK.AVG(3,{1,2,2,3})", "1"),
        ("RANK.AVG(2,{1,2,2,3},1)", "2.5"),
        ("PROB({1,2,3},{0.2,0.3,0.5},2,3)", "0.8"),
        ("PROB({1,2,3},{0.2,0.3,0.5},2)", "0.3"),
        // The chances have to add up to one.
        ("PROB({1,2},{0.2,0.3},1,2)", "#NUM!"),
    ]);
}

#[test]
fn least_squares_fits_a_line_and_a_curve() {
    check(&[
        // LINEST lays its coefficients out backwards: the slope, then the
        // intercept. An INDEX that cannot read a one-row array would need
        // these are tested here rather than as fixtures.
        ("INDEX(LINEST({1,2,3},{2,4,7}),1)", "0.3947368421052633"),
        ("INDEX(LINEST({1,2,3},{2,4,7}),2)", "0.2894736842105239"),
        // Pinned through the origin, the fit is the ratio of the sums:
        // 60/14 for y = {6,9,12} on x = {1,2,3}, with no intercept left.
        (
            "ROUND(INDEX(LINEST({6,9,12},{1,2,3},FALSE),1),9)",
            "4.285714286",
        ),
        ("INDEX(LINEST({6,9,12},{1,2,3},FALSE),2)", "0"),
        // LOGEST fits y = b * m^x, so {1,2,4} doubling gives m = 2, b = 0.5.
        ("ROUND(INDEX(LOGEST({1,2,4},{1,2,3}),1),9)", "2"),
        ("ROUND(INDEX(LOGEST({1,2,4},{1,2,3}),2),9)", "0.5"),
        // An exponential curve cannot pass through zero.
        ("LOGEST({1,0,4},{1,2,3})", "#NUM!"),
        ("GROWTH({1,-2,4},{1,2,3})", "#NUM!"),
        // With no degrees of freedom left the coefficients still stand.
        ("INDEX(LINEST({1,2},{1,2},TRUE,TRUE),1,1)", "1"),
        // A perfect fit through more points than it needs.
        ("ROUND(INDEX(TREND({3,5,7,9},{1,2,3,4},{5,6}),1,2),9)", "13"),
    ]);
}

#[test]
fn the_array_functions_reshape_what_they_are_given() {
    check(&[
        // TOROW and TOCOL flatten; treats them as scalars, so these
        // are tested here rather than as fixtures.
        ("COLUMNS(TOROW({1,2;3,4}))", "4"),
        ("ROWS(TOROW({1,2;3,4}))", "1"),
        ("ROWS(TOCOL({1,2;3,4}))", "4"),
        ("INDEX(TOROW({1,2;3,4}),1,3)", "3"),
        // XMATCH, which does not have: exact, then the next smaller
        // and the next larger.
        ("XMATCH(2,{1,2,3})", "2"),
        ("XMATCH(2.5,{1,2,3},-1)", "2"),
        ("XMATCH(2.5,{1,2,3},1)", "3"),
        ("XMATCH(9,{1,2,3})", "#N/A"),
        // A wildcard match is mode 2, and a negative direction searches back.
        ("XMATCH(\"b*\",{\"aa\",\"bb\",\"bc\"},2)", "2"),
        ("XMATCH(1,{1,2,1},0,-1)", "3"),
        // Ragged stacks are padded rather than refused.
        ("COLUMNS(VSTACK({1,2},{3}))", "2"),
        ("INDEX(VSTACK({1,2},{3}),2,2)", "#N/A"),
        // Nothing left is #CALC!, which is what a spilled array says.
        ("FILTER({1;2;3},{FALSE;FALSE;FALSE})", "#CALC!"),
        ("FILTER({1;2;3},{FALSE;FALSE;FALSE},\"none\")", "\"none\""),
    ]);
}

#[test]
fn references_can_be_built_and_moved_while_the_formula_runs() {
    check(&[
        // INDIRECT parses its text as a formula and then insists the result
        // is a reference, so arithmetic in the string is #REF! rather than a
        // quietly computed number.
        ("INDIRECT(\"A1\")", "1"),
        ("INDIRECT(\"D\"&2)", "20"),
        ("SUM(INDIRECT(\"A1:A3\"))", "6"),
        ("INDIRECT(\"1+1\")", "#REF!"),
        ("INDIRECT(\"nonsense\")", "#REF!"),
        // A defined name that is a reference is followed; one that is a
        // constant is no reference to follow.
        ("SUM(INDIRECT(\"Числа\"))", "6"),
        ("INDIRECT(\"Ставка\")", "#REF!"),
        // What INDIRECT reads is cells, whose text an aggregate skips.
        ("SUM(INDIRECT(\"A1:A3\"),INDIRECT(\"A1\"))", "7"),
        // R1C1 notation is a different language, and not one this reads.
        ("INDIRECT(\"A1\",FALSE)", "#REF!"),
        ("SUM(INDIRECT(\"Data!A1:A3\"))", "6"),
        // OFFSET moves the rectangle and then resizes it.
        ("OFFSET(A1,1,0)", "2"),
        ("SUM(OFFSET(A1,0,0,3,1))", "6"),
        ("SUM(OFFSET(A1:A2,1,0))", "5"),
        // A negative size grows the other way from the corner.
        ("SUM(OFFSET(A3,0,0,-3,1))", "6"),
        ("COLUMNS(OFFSET(A1,0,0,3,2))", "2"),
        // Off the sheet is what #REF! is for.
        ("OFFSET(A1,-1,0)", "#REF!"),
        ("SUM(OFFSET(A1,0,0,-2,1))", "#REF!"),
        ("OFFSET(A1,0,0,0,1)", "#REF!"),
    ]);
}

#[test]
fn complex_numbers_are_text_that_arithmetic_understands() {
    // Checked against excelize: the complex functions need a package that
    // no oracle at hand can answer for.
    check(&[
        ("COMPLEX(3,4)", "\"3+4i\""),
        ("COMPLEX(3,-4)", "\"3-4i\""),
        ("COMPLEX(3,0)", "\"3\""),
        // One times i is written without the one. excelize says "1i"; Excel's
        // own documentation gives COMPLEX(0,1) as i.
        ("COMPLEX(0,1)", "\"i\""),
        ("COMPLEX(0,-1)", "\"-i\""),
        ("COMPLEX(1,1,\"j\")", "\"1+j\""),
        ("IMREAL(\"3+4i\")", "3"),
        ("IMAGINARY(\"3+4i\")", "4"),
        ("IMAGINARY(\"i\")", "1"),
        ("IMABS(\"3+4i\")", "5"),
        ("IMSUM(\"3+4i\",\"5-3i\")", "\"8+i\""),
        ("IMSUB(\"3+4i\",\"1+1i\")", "\"2+3i\""),
        ("IMPRODUCT(\"3+4i\",\"5-3i\")", "\"27+11i\""),
        ("IMDIV(\"3+4i\",\"1+1i\")", "\"3.5+0.5i\""),
        ("IMSQRT(\"3+4i\")", "\"2+i\""),
        // Fifteen significant digits, so the polar form's last-bit noise does
        // not show: (2+3i)^2 is exactly -5+12i.
        ("IMPOWER(\"2+3i\",2)", "\"-5+12i\""),
        ("IMCONJUGATE(\"3+4i\")", "\"3-4i\""),
        ("IMDIV(\"1\",\"0\")", "#NUM!"),
        ("IMREAL(\"nonsense\")", "#NUM!"),
    ]);
}

#[test]
fn facts_about_a_cell_and_the_workbook() {
    // SHEET, SHEETS and CELL are not in so these are checked
    // against excelize where it has them and against Excel's documentation
    // where it does not.
    check(&[
        ("SHEET()", "1"),
        ("SHEET(\"Другой лист\")", "2"),
        ("SHEET(\"nowhere\")", "#N/A"),
        ("SHEETS()", "2"),
        ("CELL(\"row\",B3)", "3"),
        ("CELL(\"col\",B3)", "2"),
        ("CELL(\"address\",B3)", "\"$B$3\""),
        // `v` for a value, `l` for text, `b` for a cell with nothing in it.
        ("CELL(\"type\",A1)", "\"v\""),
        ("CELL(\"type\",B1)", "\"l\""),
        ("CELL(\"type\",Z99)", "\"b\""),
        // What a cell is drawn like is not something this can answer.
        ("CELL(\"format\",A1)", "#VALUE!"),
        // A negative amount goes in brackets, which is where an accountant
        // puts it; Excel writes a minus sign instead.
        ("DOLLAR(-1234.567)", "\"($1,234.57)\""),
    ]);
}

#[test]
fn matrices_sequences_and_the_rest_of_the_rounding() {
    check(&[
        // The matrix functions need support no oracle at hand can
        // supply, so these come from excelize and from the arithmetic itself:
        // the inverse of {1,2;3,4} is {-2,1;1.5,-0.5}.
        ("MDETERM({1,2;3,4})", "-2"),
        ("MDETERM({2,0,0;0,3,0;0,0,4})", "24"),
        ("INDEX(MINVERSE({1,2;3,4}),1,1)", "-2"),
        ("INDEX(MINVERSE({1,2;3,4}),2,1)", "1.5"),
        // A matrix with no inverse is a numeric failure.
        ("MINVERSE({1,2;2,4})", "#NUM!"),
        ("INDEX(MMULT({1,2;3,4},{1,0;0,1}),2,1)", "3"),
        ("INDEX(MUNIT(3),2,2)", "1"),
        ("INDEX(MUNIT(3),1,2)", "0"),
        ("INDEX(SEQUENCE(2,3),2,1)", "4"),
        ("ROWS(SEQUENCE(2,3))", "2"),
        ("INDEX(SEQUENCE(3,1,10,5),3)", "20"),
        ("DECIMAL(\"FF\",16)", "255"),
        ("AGGREGATE(9,0,A1:A3)", "6"),
        // Option 6 passes over the errors in the range rather than reporting
        // them, which is the whole point of AGGREGATE over SUBTOTAL.
        ("AGGREGATE(9,6,A1:C3)", "6"),
        ("SUM(A1:C3)", "#DIV/0!"),
        // FLOOR.MATH with the mode set rounds a negative number towards zero,
        // and CEILING.MATH away from it. excelize has the two the wrong way.
        ("FLOOR.MATH(-5.5,1,1)", "-5"),
        ("CEILING.MATH(-5.5,1,1)", "-6"),
    ]);
}

#[test]
fn the_last_of_the_engineering_and_the_reshaping() {
    check(&[
        // The series is exact where a fitted table is not: J0(1) is
        // 0.76519768655797, and excelize agrees with us.
        ("ROUND(BESSELJ(1,0),12)", "0.765197686558"),
        ("ROUND(BESSELI(1.5,1),12)", "0.981666428578"),
        // Temperature needs an offset, not just a scale.
        ("CONVERT(68,\"F\",\"C\")", "20"),
        ("ROUND(CONVERT(100,\"C\",\"F\"),9)", "212"),
        ("CONVERT(0,\"C\",\"K\")", "273.15"),
        // Between kinds is not a conversion at all.
        ("CONVERT(1,\"m\",\"C\")", "#N/A"),
        ("CONVERT(2.5,\"ft\",\"sec\")", "#N/A"),
        // The wrapping and growing functions, which neither engine has.
        ("INDEX(WRAPROWS({1,2,3,4,5},3),2,1)", "4"),
        ("INDEX(WRAPROWS({1,2,3,4,5},3),2,3)", "#N/A"),
        ("INDEX(WRAPROWS({1,2,3,4,5},3,0),2,3)", "0"),
        ("COLUMNS(WRAPCOLS({1,2,3,4},2))", "2"),
        ("INDEX(EXPAND({1,2},2,3,0),2,3)", "0"),
        ("ROWS(EXPAND({1,2},2,3,0))", "2"),
        // Growing is what EXPAND does; shrinking is an error.
        ("EXPAND({1,2,3},1,2)", "#VALUE!"),
        // RAND cannot be pinned to a value, only to its range.
        ("AND(RAND()>=0,RAND()<1)", "TRUE"),
        ("AND(RANDBETWEEN(5,5)=5)", "TRUE"),
        ("ROWS(RANDARRAY(3,2))", "3"),
    ]);
}

#[test]
fn math_rounds_the_way_excel_does() {
    check(&[
        // Half away from zero, where Rust's own `round` would go to even.
        ("ROUND(2.5,0)", "3"),
        ("ROUND(-2.5,0)", "-3"),
        ("ROUND(1.2345,2)", "1.23"),
        ("ROUNDUP(1.001,2)", "1.01"),
        ("ROUNDDOWN(-1.999,2)", "-1.99"),
        ("TRUNC(-1.9)", "-1"),
        ("INT(-1.9)", "-2"),
        // The result takes the sign of the divisor.
        ("MOD(-3,2)", "1"),
        ("MOD(3,-2)", "-1"),
        ("MOD(3,0)", "#DIV/0!"),
        ("ABS(-2)", "2"),
        ("SIGN(-2)", "-1"),
        ("SQRT(9)", "3"),
        ("SQRT(-1)", "#NUM!"),
        ("POWER(2,10)", "1024"),
        ("LOG(8,2)", "3"),
        ("LOG10(1000)", "3"),
        ("CEILING(2.1,1)", "3"),
        ("FLOOR(2.9,1)", "2"),
    ]);
}

/// A function whose parameter takes one value, handed an array, answers
/// element by element - what Excel calls lifting, and what an array formula
/// such as `PRODUCT(IF(ISNUMBER(r), r, 1))` rests on. Which parameters lift is
/// the value class of the function's signature; a reference or array
/// parameter takes the array whole.
#[test]
fn a_one_value_parameter_lifts_over_an_array() {
    check(&[
        ("ABS({-1,2})", "{1 2}"),
        ("LEN({\"a\",\"bb\"})", "{1 2}"),
        ("ISNUMBER({1,\"a\"})", "{TRUE FALSE}"),
        ("ROUND({1.26,2.34},1)", "{1.3 2.3}"),
        ("MATCH({2,3},{1,2,3},0)", "{2 3}"),
        ("LARGE({5,3,9},{1,2})", "{9 5}"),
        // Reference and array parameters take the array whole.
        ("SUM({1,2})", "3"),
        ("SUMPRODUCT({1,2},{3,4})", "11"),
        // `TYPE` asks about the array itself.
        ("TYPE({1,2})", "64"),
        // An array of conditions picks element by element, from both branches.
        ("IF(ISNUMBER({1,\"a\"}),{2,3},1)", "{2 1}"),
        ("IF({TRUE,FALSE},5)", "{5 FALSE}"),
        ("PRODUCT(IF(ISNUMBER({2,\"n/a\",3}),{2,\"n/a\",3},1))", "6"),
        // Each element of an array is caught on its own.
        ("IFERROR({1,#N/A},0)", "{1 0}"),
        // Inside an array, only numbers count, as inside a reference.
        ("SUM({1,\"2\",TRUE})", "1"),
        ("SUM(1,\"2\",TRUE)", "4"),
    ]);
}

/// Cases where a workbook saved by Excel 2016 cached a different answer than
/// the engine gave: `tests/corpus/Excel_Формулы_Справочник_676_формул.xlsx`.
#[test]
fn what_an_excel_formula_reference_cached() {
    check(&[
        // Approximate lookups halve the list as Excel does, so an unsorted
        // one lands where Excel's answer does, not on the first fit.
        ("MATCH(58000,{50000,60000,55000,70000,45000,65000},1)", "5"),
        ("MATCH(3,{1,2,3,3,4},1)", "4"),
        ("MATCH(3,{5,4,3,1},-1)", "3"),
        ("MATCH(0,{1,2},1)", "#N/A"),
        // A range that is an error fails the count; an error in a range does
        // not.
        ("COUNTIF(INDIRECT(\"OD-\"),\"<5\")", "#REF!"),
        ("SUMIF({1,2},\">0\",INDIRECT(\"OD-\"))", "#REF!"),
        // An error handed to a function is its answer, whichever error it
        // is - not a #VALUE! for failing to be a number.
        ("RANDBETWEEN(1/0,5)", "#DIV/0!"),
        ("ROUND(#REF!,2)", "#REF!"),
        ("COUNTIF(INDIRECT(\"OD-\"),SQRT(-1))", "#REF!"),
        // The functions that look at errors still do; the counts skip them.
        ("ISERROR(1/0)", "TRUE"),
        ("ERROR.TYPE(#REF!)", "4"),
        ("COUNT(1,#REF!)", "1"),
        // Excel's calendar ends on 31 December 9999: a date past it is
        // #NUM!, and so is a count of working days that would land there.
        ("NETWORKDAYS(1,4E+8)", "#NUM!"),
        ("YEAR(2958466)", "#NUM!"),
        ("YEAR(2958465)", "9999"),
        ("WORKDAY(1,1E+9)", "#NUM!"),
        ("EDATE(1E+9,1)", "#NUM!"),
        ("EDATE(1,2147483646)", "#NUM!"),
        ("EOMONTH(1,-2147483646)", "#NUM!"),
        ("EDATE(2958465,1)", "#NUM!"),
        ("EDATE(2958100,12)", "2958465"),
        // Found by fuzzing: a fractional position under one is the whole row.
        ("SUM(INDEX({1,2;3,4},0.2,1))", "4"),
        ("INDEX({1,2;3,4},1.9,2.7)", "2"),
        // Found by fuzzing: an array a formula asks for is capped at what a
        // reference may pull in, four million cells, rather than allocated.
        ("EXPAND(1,50001,20250)", "#NUM!"),
        ("ROWS(RANDARRAY(1E+6,1E+6))", "#NUM!"),
        ("SUM(SEQUENCE(1,5000)*SEQUENCE(5000,1))", "#NUM!"),
        ("ROWS(EXPAND(1,3,2))", "3"),
        // Found by fuzzing: a large number of trials is searched, not walked.
        ("CRITBINOM(77777776,0.5,0.5)", "38888888"),
        ("BINOM.INV(2000,0.3,0.9)", "626"),
        ("BINOM.INV(1000,0.3,0.9)", "319"),
        ("ROUND(BINOM.DIST.RANGE(5000,0.5,0,2500),6)", "0.505642"),
        ("ROUND(BINOM.DIST.RANGE(60,0.75,45,50),6)", "0.52363"),
        ("HYPGEOM.DIST(2E+7,4E+7,3E+7,5E+7,TRUE)", "#NUM!"),
        // Found by fuzzing: a size past any sheet is off the sheet, not an
        // overflow on the way there.
        ("SUM(OFFSET(A1,0,0,1E+99))", "#REF!"),
        ("SUM(OFFSET(A1,-1E+99,0))", "#REF!"),
        ("SUM(OFFSET(A1,0,0,-1E+99,-1E+99))", "#REF!"),
        // Only values of the needle's kind are searched: blanks and numbers
        // in a row of names are stepped over.
        ("MATCH(\"b\",{\"a\",1,\"b\",2,\"c\"})", "3"),
        // Saving up from nothing is a present value of zero.
        ("ROUND(NPER(8%%/12,-10000,0,1829460.27),6)", "181.845436"),
        // Days since the start's day of the month, in the end's own month.
        ("DATEDIF(DATE(2020,1,1),DATE(2024,6,15),\"MD\")", "14"),
        ("DATEDIF(DATE(2020,1,15),DATE(2024,3,10),\"MD\")", "24"),
        // A date that is not a number fails the call instead of shifting
        // the rest.
        ("XNPV(0.1,{-100,50},{\"2020\",\"2021\"})", "#VALUE!"),
        // A discount that leaves the bill worth nothing has no yield.
        ("TBILLEQ(DATE(2024,1,1),DATE(2024,7,1),95)", "#NUM!"),
        ("ROUND(IRR({-100,30,40,50}),15)", "0.08896339469335"),
    ]);
}

#[test]
fn arrays_broadcast_element_by_element() {
    check(&[
        ("{1,2;3,4}", "{1 2; 3 4}"),
        ("{1,2}*2", "{2 4}"),
        ("A1:A3*2", "{2; 4; 6}"),
        ("SUM(A1:A3*2)", "12"),
        ("SUM({1,2}+{10,20})", "33"),
    ]);
}

#[test]
fn unknown_names_and_functions() {
    check(&[
        ("NoSuchFunction(1)", "#NAME?"),
        ("SomeDefinedName", "#NAME?"),
        ("Data!SomeDefinedName", "#NAME?"),
        ("1+", "#NAME?"),
    ]);
}

#[test]
fn a_formula_cell_is_computed_when_something_reads_it() {
    let mut wb = Spreadsheet::empty();
    let mut ws = Worksheet::new("S").unwrap();
    ws.set(at("A1"), 2.0);
    ws.entry(at("A2")).value = CellValue::Formula {
        formula: "A1*3".into(),
        cached: None,
    };
    ws.entry(at("A3")).value = CellValue::Formula {
        formula: "A2+1".into(),
        cached: None,
    };
    // A cell that refers to itself has no value to compute.
    ws.entry(at("B1")).value = CellValue::Formula {
        formula: "B1+1".into(),
        cached: None,
    };
    wb.add_sheet(ws).unwrap();

    let mut engine = Engine::new(&wb);
    assert_eq!(show(&engine.cell(0, at("A3"))), "7", "through two formulas");
    assert_eq!(
        show(&engine.eval(Origin::new(0, at("C1")), "SUM(A1:A3)")),
        "15"
    );
    assert_eq!(
        show(&engine.cell(0, at("B1"))),
        "#REF!",
        "a circular reference is reported, not looped over"
    );
}

#[test]
fn dates_are_numbers_on_excels_own_calendar() {
    check(&[
        ("DATE(2025,1,1)", "45658"),
        // Excel rolls the parts over rather than refusing them.
        ("DATE(2008,14,2)", "39846"),
        ("DATE(2008,1,35)", "39482"),
        ("DATE(2008,-3,2)", "39327"),
        // A year below 1900 counts from 1900, so this is 2 January 2008.
        ("DATE(108,1,2)", "39449"),
        ("DATE(10000,1,1)", "#NUM!"),
        ("YEAR(45658)", "2025"),
        ("MONTH(45658)", "1"),
        ("DAY(45658)", "1"),
        // Serial 0 is Excel's fictional "0 January 1900".
        ("YEAR(0)", "1900"),
        ("DAY(0)", "0"),
        ("TIME(12,0,0)", "0.5"),
        ("TIME(25,0,0)", "0.041666666666666664"),
        ("HOUR(0.5)", "12"),
        ("EDATE(45658,1)", "45689"),
        // One month after 31 January is the end of February, not the 31st.
        ("EDATE(45688,1)", "45716"),
        ("EOMONTH(45658,0)", "45688"),
        ("DAYS(45658,45657)", "1"),
        ("WEEKDAY(45658)", "4"),
        ("WEEKDAY(45658,2)", "3"),
        ("ISOWEEKNUM(45658)", "1"),
        ("NETWORKDAYS(DATE(2025,1,1),DATE(2025,1,31))", "23"),
        ("WORKDAY(DATE(2025,1,1),5)", "45665"),
        ("YEARFRAC(DATE(2025,1,1),DATE(2025,7,1))", "0.5"),
        // The weekend of the `.INTL` pair is named, not assumed: code 11 is
        // Sunday alone, and the mask starts on Monday.
        ("NETWORKDAYS.INTL(DATE(2025,1,1),DATE(2025,1,31),11)", "27"),
        (
            "NETWORKDAYS.INTL(DATE(2025,1,1),DATE(2025,1,31),\"0000011\")",
            "23",
        ),
        (
            "NETWORKDAYS.INTL(DATE(2025,1,1),DATE(2025,1,31),\"1000000\")",
            "27",
        ),
        ("NETWORKDAYS.INTL(DATE(2025,1,1),DATE(2025,1,31))", "23"),
        ("WORKDAY.INTL(DATE(2025,1,1),5,11)", "45664"),
        ("WORKDAY.INTL(DATE(2025,1,1),5)", "45665"),
        // Every day a weekend would leave no working day to count.
        (
            "NETWORKDAYS.INTL(DATE(2025,1,1),DATE(2025,1,31),\"1111111\")",
            "#VALUE!",
        ),
        (
            "NETWORKDAYS.INTL(DATE(2025,1,1),DATE(2025,1,31),8)",
            "#NUM!",
        ),
        ("DATEDIF(DATE(2000,1,15),DATE(2025,3,10),\"Y\")", "25"),
    ]);
}

#[test]
fn the_phantom_29_february_1900_is_a_real_day_here() {
    // Excel's calendar has a 29 February 1900 that never existed, and every
    // date function walks that calendar. Converting to a real calendar date
    // so loses the day: it answers 28 February for serial 60 and repeats the
    // weekday of serial 59.
    check(&[
        ("DAY(60)", "29"),
        ("MONTH(60)", "2"),
        ("WEEKDAY(59)", "3"),
        ("WEEKDAY(60)", "4"),
        ("WEEKDAY(61)", "5"),
        // Serial 1 is a Sunday, which is where the whole week count starts.
        ("WEEKDAY(1)", "1"),
    ]);
}

#[test]
fn criteria_functions_match_the_way_excel_matches() {
    check(&[
        ("COUNTIF(A1:A3,\">1\")", "2"),
        ("COUNTIF(A1:A3,2)", "1"),
        ("COUNTIF(B1:B3,\"x\")", "1"),
        ("COUNTIF(B1:B3,\"X\")", "1"),
        // A wildcard tests text and nothing else: the boolean in B2 and the
        // empty B3 do not match, though counts both.
        ("COUNTIF(B1:B3,\"*\")", "1"),
        ("COUNTIF(B1:B3,\"?\")", "1"),
        ("COUNTIF(A1:A3,\"*\")", "0"),
        // An error cell holds something, so it is not empty.
        ("COUNTIF(A1:D3,\"<>\")", "9"),
        ("COUNTIFS(A1:A3,\">1\",D1:D3,\"<30\")", "1"),
        ("SUMIF(A1:A3,\">1\")", "5"),
        ("SUMIF(A1:A3,\">1\",D1:D3)", "50"),
        ("SUMIFS(D1:D3,A1:A3,\">1\")", "50"),
        ("AVERAGEIF(A1:A3,\">1\",D1:D3)", "25"),
        ("SUMPRODUCT(A1:A3,D1:D3)", "140"),
        ("LARGE(A1:A3,1)", "3"),
        ("LARGE(A1:A3,4)", "#NUM!"),
        ("SMALL(A1:A3,1)", "1"),
        ("RANK(2,A1:A3)", "2"),
        ("RANK(9,A1:A3)", "#N/A"),
        ("STDEV(A1:A3)", "1"),
        ("VARP(A1:A3)", "0.6666666666666666"),
    ]);
}

#[test]
fn defined_names_stand_for_what_they_were_given() {
    check(&[
        ("Ставка", "0.15"),
        ("Ставка*2", "0.3"),
        ("ставка", "0.15"),
        ("SUM(Числа)", "6"),
        ("INDEX(Числа,2)", "2"),
        // Scoped to the sheet the formula sits on, which is the first one.
        ("Свой", "111"),
        ("'Другой лист'!Свой", "222"),
        ("Петля", "#REF!"),
    ]);
}

#[test]
fn the_japanese_width_pair_is_reversible() {
    check(&[
        // ASCII is a fixed offset apart, and the ideographic space is its own
        // case both ways.
        ("ASC(\"ＡＢＣ\")", "\"ABC\""),
        ("DBCS(\"ABC\")", "\"ＡＢＣ\""),
        ("LEN(ASC(\"　\"))", "1"),
        ("DBCS(\" \")", "\"　\""),
        // Katakana comes from a table, not from arithmetic.
        ("ASC(\"カタカナ\")", "\"ｶﾀｶﾅ\""),
        ("DBCS(\"ｶﾀｶﾅ\")", "\"カタカナ\""),
        // A voiced character is one full-width but two half-width.
        ("ASC(\"ガ\")", "\"ｶﾞ\""),
        ("DBCS(\"ｶﾞ\")", "\"ガ\""),
        ("ASC(\"パ\")", "\"ﾊﾟ\""),
        ("DBCS(\"ﾊﾟ\")", "\"パ\""),
        ("LEN(ASC(\"ガ\"))", "2"),
        // A mark after a character that has no voiced form stays a character
        // of its own.
        ("LEN(DBCS(\"ｱﾞ\"))", "2"),
        // What has no twin passes through.
        ("ASC(\"漢字\")", "\"漢字\""),
        ("JIS(\"ｶﾅ\")", "\"カナ\""),
    ]);
}

#[test]
fn the_byte_named_text_functions_are_their_ordinary_twins() {
    // `LENB` and its kin count bytes in a double-byte code page. Text here is
    // UTF-8 and carries no code page, so they answer as the plain forms do -
    // which is what a reader has to do too, mapping both names to one function.
    check(&[
        ("LENB(\"abc\")", "3"),
        ("LENB(\"日本\")", "2"),
        ("LEFTB(\"abcdef\",2)", "\"ab\""),
        ("MIDB(\"abcdef\",2,3)", "\"bcd\""),
        ("RIGHTB(\"abcdef\",2)", "\"ef\""),
        ("FINDB(\"c\",\"abc\")", "3"),
        ("SEARCHB(\"C\",\"abc\")", "3"),
        ("REPLACEB(\"abcdef\",2,3,\"X\")", "\"aXef\""),
    ]);
}

#[test]
fn depreciation_over_a_span_and_the_french_pair() {
    // `VDB` checked against excelize, which agrees on all seven, fractional
    // periods and `no_switch` included.
    check(&[
        ("VDB(2400,300,10,0,1)", "480"),
        ("VDB(2400,300,10,0,0.875)", "420"),
        ("VDB(2400,300,120,0,1)", "40"),
        ("VDB(2400,300,10,6,7)", "125.82912000000002"),
        ("VDB(2400,300,10,6,7,1.5)", "151.28970937499997"),
        ("VDB(2400,300,10,0,10)", "2100"),
        // `no_switch` keeps the declining balance where straight line is faster.
        ("VDB(2400,300,10,0,3,2,TRUE)", "1171.2"),
    ]);
    // `AMORDEGRC` and `AMORLINC`: the first two are Microsoft's own documented
    // examples, and a second implementation agrees with the rest. excelize has
    // neither function.
    check(&[
        ("AMORLINC(2400,39679,39813,300,1,0.15,1)", "360"),
        ("AMORDEGRC(2400,39679,39813,300,1,0.15,1)", "776"),
        // Period 0 is the prorated first period.
        (
            "AMORLINC(2400,39679,39813,300,0,0.15,1)",
            "131.44316044074174",
        ),
        ("AMORDEGRC(2400,39679,39813,300,0,0.15,1)", "330"),
        ("AMORLINC(2400,39679,39813,300,2,0.15,1)", "360"),
        ("AMORDEGRC(2400,39679,39813,300,2,0.15,1)", "485"),
        ("AMORDEGRC(2400,39679,39813,300,3,0.15,1)", "303"),
        // Basis 0 measures the first period the plain 30/360 way.
        (
            "AMORLINC(2400,39679,39813,300,0,0.15,0)",
            "131.99999999999997",
        ),
    ]);
}

#[test]
fn the_coupon_calendar_walks_back_from_maturity() {
    // excelize agrees on all but the two marked below, where it is the one
    // that is wrong.
    check(&[
        ("COUPPCD(DATE(2011,1,25),DATE(2011,11,15),2,1)", "40497"),
        ("COUPNCD(DATE(2011,1,25),DATE(2011,11,15),2,1)", "40678"),
        ("COUPDAYBS(DATE(2011,1,25),DATE(2011,11,15),2,1)", "71"),
        ("COUPDAYS(DATE(2011,1,25),DATE(2011,11,15),2,1)", "181"),
        ("COUPDAYSNC(DATE(2011,1,25),DATE(2011,11,15),2,1)", "110"),
        ("COUPNUM(DATE(2011,1,25),DATE(2011,11,15),2,1)", "2"),
        ("COUPDAYBS(DATE(2011,1,25),DATE(2011,11,15),2,0)", "70"),
        ("COUPDAYS(DATE(2011,1,25),DATE(2011,11,15),2,0)", "180"),
        ("COUPDAYS(DATE(2011,1,25),DATE(2011,11,15),2,3)", "182.5"),
        ("COUPDAYS(DATE(2011,1,25),DATE(2011,11,15),4,1)", "92"),
        ("COUPNUM(DATE(2007,1,25),DATE(2008,11,15),4)", "8"),
        // A maturity on the last day of a month keeps to the last day, so a
        // bond maturing on 31 August pays on 29 February in a leap year.
        // excelize answers 40786 and 40968 here, which is its own bug.
        ("COUPPCD(DATE(2012,2,29),DATE(2015,8,31),2)", "40968"),
        ("COUPNCD(DATE(2012,2,29),DATE(2015,8,31),2)", "41152"),
        // A settlement on or after the maturity is not a coupon period.
        ("COUPNUM(DATE(2011,11,15),DATE(2011,11,15),2)", "#NUM!"),
        ("COUPDAYS(DATE(2011,1,25),DATE(2011,11,15),3)", "#NUM!"),
    ]);
}

#[test]
fn securities_price_and_yield() {
    // All of these are Microsoft's own documented examples, and excelize
    // agrees with every one to fifteen digits.
    check(&[
        (
            "PRICE(DATE(2008,2,15),DATE(2017,11,15),0.0575,0.065,100,2,0)",
            "94.63436162132213",
        ),
        (
            "PRICEDISC(DATE(2008,2,16),DATE(2008,3,1),0.0525,100,2)",
            "99.79583333333333",
        ),
        (
            "PRICEMAT(DATE(2008,2,15),DATE(2008,4,13),DATE(2007,11,11),0.061,0.061,0)",
            "99.98449887555694",
        ),
        (
            "DISC(DATE(2007,1,25),DATE(2007,6,15),97.975,100,1)",
            "0.05242021276595771",
        ),
        (
            "INTRATE(DATE(2008,2,15),DATE(2008,5,15),1000000,1014420,2)",
            "0.0576800000000004",
        ),
        (
            "RECEIVED(DATE(2008,2,15),DATE(2008,5,15),1000000,0.0575,2)",
            "1014584.6544071021",
        ),
        (
            "ACCRINTM(DATE(2008,4,1),DATE(2008,6,15),0.1,1000,3)",
            "20.54794520547945",
        ),
        (
            "ACCRINT(DATE(2008,3,1),DATE(2008,8,31),DATE(2008,5,1),0.1,1000,2,0)",
            "16.666666666666664",
        ),
        (
            "YIELDDISC(DATE(2008,2,16),DATE(2008,3,1),99.795,100,2)",
            "0.05282257198685834",
        ),
        (
            "YIELDMAT(DATE(2008,3,15),DATE(2008,11,3),DATE(2007,11,8),0.0625,100.0123,0)",
            "0.06095433369153867",
        ),
        (
            "TBILLEQ(DATE(2008,3,31),DATE(2008,6,1),0.0914)",
            "0.09415149356594302",
        ),
        ("TBILLPRICE(DATE(2008,3,31),DATE(2008,6,1),0.09)", "98.45"),
        (
            "TBILLYIELD(DATE(2008,3,31),DATE(2008,6,1),98.45)",
            "0.09141696292534264",
        ),
        // `YIELD` has no closed form and is bisected against `PRICE`.
        (
            "YIELD(DATE(2008,2,15),DATE(2016,11,15),0.0575,95.04287,100,2,0)",
            "0.0650000068807548",
        ),
        (
            "YIELD(DATE(2010,1,1),DATE(2012,1,1),0.05,100,100,4,0)",
            "0.050000000000000266",
        ),
    ]);
}

#[test]
fn duration_weights_each_payment_by_when_it_arrives() {
    // Microsoft documents 5.993775 and 5.7356702 for this bond. excelize
    // answers 5.99195 and 5.73392 on basis 1, which is its own bug: it agrees
    // with us to fifteen digits on basis 0.
    check(&[
        (
            "DURATION(DATE(2008,1,1),DATE(2016,1,1),0.08,0.09,2,1)",
            "5.993774955545185",
        ),
        (
            "MDURATION(DATE(2008,1,1),DATE(2016,1,1),0.08,0.09,2,1)",
            "5.735669813918838",
        ),
        (
            "DURATION(DATE(2010,7,1),DATE(2020,1,1),0.0575,0.065,2,0)",
            "7.383947618600286",
        ),
        (
            "MDURATION(DATE(2010,7,1),DATE(2020,1,1),0.0575,0.065,2,0)",
            "7.151523117288413",
        ),
    ]);
}

#[test]
fn odd_first_and_last_periods() {
    // Microsoft's documented examples, and excelize agrees with all four.
    check(&[
        (
            "ODDFPRICE(DATE(2008,11,11),DATE(2021,3,1),DATE(2008,10,15),DATE(2009,3,1),0.0785,0.0625,100,2,1)",
            "113.59771747407882",
        ),
        (
            "ODDFYIELD(DATE(2008,12,11),DATE(2021,4,1),DATE(2008,10,15),DATE(2009,4,1),0.0575,84.5,100,2,0)",
            "0.0772299555190823",
        ),
        (
            "ODDLPRICE(DATE(2008,2,7),DATE(2008,6,15),DATE(2007,10,15),0.0375,0.0405,100,2,0)",
            "99.87828601472134",
        ),
        (
            "ODDLYIELD(DATE(2008,4,20),DATE(2008,6,15),DATE(2007,12,24),0.0375,99.875,100,2,0)",
            "0.045192235629168256",
        ),
    ]);
}

#[test]
fn an_odd_period_that_is_not_odd_prices_like_an_ordinary_one() {
    // The strongest check these have: when the first or last period turns out
    // to line up with the schedule after all, the odd formula has to agree
    // with `PRICE`, which is checked against Microsoft's own examples above.
    check(&[
        (
            "PRICE(DATE(2008,3,15),DATE(2010,1,15),0.05,0.06,100,2,0)",
            "98.27987819620141",
        ),
        (
            "ODDFPRICE(DATE(2008,3,15),DATE(2010,1,15),DATE(2008,1,15),DATE(2008,7,15),0.05,0.06,100,2,0)",
            "98.27987819620142",
        ),
        // With one period left Excel discounts at simple interest, which is
        // also what `ODDLPRICE` does: the two meet exactly.
        (
            "PRICE(DATE(2008,3,15),DATE(2008,7,15),0.05,0.06,100,2,0)",
            "99.65686274509804",
        ),
        (
            "ODDLPRICE(DATE(2008,3,15),DATE(2008,7,15),DATE(2008,1,15),0.05,0.06,100,2,0)",
            "99.65686274509804",
        ),
    ]);
}

#[test]
fn lambdas_bind_names_and_travel_as_values() {
    check(&[
        // A lambda called where it is written: the parser reads the second
        // pair of brackets as an application.
        ("LAMBDA(x,x+1)(5)", "6"),
        ("LAMBDA(x,y,x*y)(3,4)", "12"),
        ("LET(a,2,b,3,a*b)", "6"),
        // An inner `LET` shadows an outer one, and gives the name back after.
        ("LET(a,2,a+LET(a,10,a))", "12"),
        // A lambda keeps what was bound where it was written.
        ("LET(k,10,LAMBDA(x,x*k)(3))", "30"),
        ("SUM(MAP({1,2,3},LAMBDA(x,x*x)))", "14"),
        ("MAP({1,2},{10,20},LAMBDA(x,y,x+y))", "{11 22}"),
        ("REDUCE(0,{1,2,3,4},LAMBDA(acc,x,acc+x))", "10"),
        ("SCAN(0,{1,2,3},LAMBDA(acc,x,acc+x))", "{1 3 6}"),
        // A row answers with one value, so `BYROW` gives a column.
        ("BYROW({1,2;3,4},LAMBDA(r,SUM(r)))", "{3; 7}"),
        ("BYCOL({1,2;3,4},LAMBDA(c,SUM(c)))", "{4 6}"),
        ("MAKEARRAY(2,3,LAMBDA(r,c,r*10+c))", "{11 12 13; 21 22 23}"),
        ("SUM(MAKEARRAY(3,3,LAMBDA(r,c,r*c)))", "36"),
    ]);
}

#[test]
fn a_lambda_used_wrongly_says_so() {
    check(&[
        // Called with the wrong number of arguments.
        ("LAMBDA(x,x+1)(1,2)", "#VALUE!"),
        // `LET` with nothing left over for a body.
        ("LET(a,1)", "#VALUE!"),
        // Something that is not a function, asked to be one.
        ("5(3)", "#CALC!"),
        // A lambda is not a value a cell can hold, so arithmetic on one is
        // `#CALC!` rather than a silent zero.
        ("LAMBDA(x,x)+1", "#CALC!"),
        // A parameter has to be a name.
        ("LAMBDA(1,2)(3)", "#VALUE!"),
        ("MAP({1,2},5)", "#CALC!"),
    ]);
}

#[test]
fn the_second_kind_bessel_functions() {
    // Known values, to the digits a double carries: these are the ones the
    // first attempt at this got plausibly wrong, which is why they waited for
    // a method that can be checked rather than a table of fitted constants.
    check(&[
        ("BESSELY(1,0)", "0.08825696421567694"),
        ("BESSELY(1,1)", "-0.7812128213002888"),
        ("BESSELK(1,0)", "0.42102443824070845"),
        ("BESSELK(1,1)", "0.6019072301972346"),
        // Higher orders come from the recurrence.
        ("BESSELY(2.5,3)", "-0.756055496753671"),
        ("BESSELK(2.5,3)", "0.268227146393444"),
        // Past the crossover the asymptotic expansion answers instead; Y0(10)
        // is 0.0556711672836 and K0(12) is 2.2008290e-6.
        ("BESSELY(10,0)", "0.05567116728363489"),
        ("BESSELK(12,0)", "0.00000220082539731821"),
        // Neither is defined at or below zero.
        ("BESSELY(0,0)", "#NUM!"),
        ("BESSELK(-1,0)", "#NUM!"),
    ]);
}

#[test]
fn thai_money_and_numerals() {
    // Thai counts in blocks of six digits, and a ten drops its "one" while a
    // one after a ten changes word.
    check(&[
        ("BAHTTEXT(1)", "\"หนึ่งบาทถ้วน\""),
        ("BAHTTEXT(11)", "\"สิบเอ็ดบาทถ้วน\""),
        ("BAHTTEXT(21)", "\"ยี่สิบเอ็ดบาทถ้วน\""),
        ("BAHTTEXT(0)", "\"ศูนย์บาทถ้วน\""),
        ("BAHTTEXT(-5)", "\"ลบห้าบาทถ้วน\""),
        ("BAHTTEXT(1000000)", "\"หนึ่งล้านบาทถ้วน\""),
        ("BAHTTEXT(1234.56)", "\"หนึ่งพันสองร้อยสามสิบสี่บาทห้าสิบหกสตางค์\""),
        ("THAIDIGIT(\"123\")", "\"๑๒๓\""),
        ("ISTHAIDIGIT(THAIDIGIT(\"42\"))", "TRUE"),
        ("ISTHAIDIGIT(\"42\")", "FALSE"),
        // Neither of this pair is documented; the unit in the name is what
        // fixes them to whole baht, and the sign follows ROUNDUP/ROUNDDOWN.
        ("ROUNDBAHTUP(1.2)", "2"),
        ("ROUNDBAHTUP(2)", "2"),
        ("ROUNDBAHTDOWN(1.8)", "1"),
        ("ROUNDBAHTUP(-1.2)", "-2"),
        ("ROUNDBAHTDOWN(-1.8)", "-1"),
    ]);
}

#[test]
fn implicit_intersection_takes_the_caller_own_row_or_column() {
    // `SINGLE` is the `@` operator: it needs the cell asking, so these are
    // computed from a chosen origin rather than through `check`.
    let wb = book();
    let mut engine = Engine::new(&wb);
    let from = |engine: &mut Engine<'_>, address: &str, formula: &str| {
        show(&engine.eval(Origin::new(0, at(address)), formula))
    };

    // Asked from B2, a column reference answers with its second row.
    assert_eq!(from(&mut engine, "B2", "SINGLE(A1:A3)"), "2");
    assert_eq!(from(&mut engine, "B3", "SINGLE(A1:A3)"), "3");
    // Asked from a row outside the reference, there is nothing to intersect.
    assert_eq!(from(&mut engine, "B9", "SINGLE(A1:A3)"), "#VALUE!");
    // A row reference intersects on the column instead.
    assert_eq!(from(&mut engine, "A5", "SINGLE(A1:D1)"), "1");
    assert_eq!(from(&mut engine, "D5", "SINGLE(A1:D1)"), "10");
    // A rectangle has no single cell, and a plain value is already single.
    assert_eq!(from(&mut engine, "B2", "SINGLE(A1:D3)"), "#VALUE!");
    assert_eq!(from(&mut engine, "B2", "SINGLE(5)"), "5");
}

#[test]
fn anchorarray_hands_back_the_whole_array_a_cell_computed() {
    // Nothing spills here: a cell whose formula gave an array shows its top
    // left value, and a plain reference reads that, as it reads any cell.
    // `ANCHORARRAY` - `F1#` - is what asks for the array behind it.
    let mut wb = book();
    let sheet = wb.sheet_mut(0).unwrap();
    sheet.set(
        at("F1"),
        CellValue::Formula {
            formula: "{1,2;3,4}".into(),
            cached: None,
        },
    );
    let mut engine = Engine::new(&wb);
    let origin = Origin::new(0, at("Z100"));
    assert_eq!(show(&engine.eval(origin, "ANCHORARRAY(F1)")), "{1 2; 3 4}");
    assert_eq!(show(&engine.eval(origin, "F1")), "1");
    assert_eq!(show(&engine.eval(origin, "SINGLE(F1)")), "1");
    // The anchor is one cell, not a range.
    assert_eq!(show(&engine.eval(origin, "ANCHORARRAY(A1:A3)")), "#REF!");
}

/// `FORECAST.ETS` and its companions, reached through the parser: the names
/// carry two dots, which no other function in the library does.
///
/// The expectations are the two shapes the model is pinned to. A series that
/// is exactly a line is continued as that line, so the tenth point of
/// `y = 3x + 7` past the data is arithmetic, not a fit; a series that repeats
/// every four points is reported as having a period of four.
#[test]
fn exponential_smoothing_reaches_the_engine() {
    let mut wb = Spreadsheet::empty();
    let mut ws = Worksheet::new("Series").unwrap();
    for row in 1..=20u32 {
        ws.set(at(&format!("A{row}")), f64::from(row));
        ws.set(at(&format!("B{row}")), f64::from(3 * row + 7));
        ws.set(
            at(&format!("C{row}")),
            [10.0, 20.0, 30.0, 40.0][(row as usize - 1) % 4],
        );
    }
    wb.add_sheet(ws).unwrap();
    let mut engine = Engine::new(&wb);
    let origin = Origin::new(0, at("Z100"));

    let line = engine.eval(origin, "FORECAST.ETS(25,B1:B20,A1:A20,0)");
    let Value::Number(n) = line else {
        panic!("expected a number, got {line:?}");
    };
    assert!((n - 82.0).abs() < 1e-6, "the line was not continued: {n}");

    assert_eq!(
        show(&engine.eval(origin, "FORECAST.ETS.SEASONALITY(C1:C20,A1:A20)")),
        "4"
    );
    // Statistic 8 is the step of the timeline, which here is one row per point.
    assert_eq!(
        show(&engine.eval(origin, "FORECAST.ETS.STAT(B1:B20,A1:A20,8,0)")),
        "1"
    );
    // A confidence interval is a width, so it is never negative, and on a
    // series the model fits exactly there is nothing to be uncertain about.
    assert_eq!(
        show(&engine.eval(origin, "ROUND(FORECAST.ETS.CONFINT(21,B1:B20,A1:A20),6)")),
        "0"
    );
    // A target before the timeline starts has nothing to forecast from.
    assert_eq!(
        show(&engine.eval(origin, "FORECAST.ETS(0,B1:B20,A1:A20)")),
        "#NUM!"
    );
}

/// The bracket grammar of a structured reference, which is its own little
/// language: the parser has to get through all of these before the evaluator
/// ever sees them.
#[test]
fn structured_references_parse_in_every_form() {
    use excelerate::formula::parser::{Expr, Structured, TablePart, parse};

    let shape = |text: &str| -> Structured {
        match parse(text).unwrap_or_else(|e| panic!("{text}: {e}")) {
            Expr::Structured(s) => s,
            Expr::Call { args, .. } => match args.into_iter().next() {
                Some(Expr::Structured(s)) => s,
                other => panic!("{text}: {other:?}"),
            },
            other => panic!("{text}: {other:?}"),
        }
    };

    let plain = shape("Sales[Amount]");
    assert_eq!(plain.table.as_deref(), Some("Sales"));
    assert_eq!(plain.part, TablePart::Data);
    assert_eq!(plain.columns, Some(("Amount".into(), None)));

    assert_eq!(shape("SUM(Sales[#All])").part, TablePart::All);
    assert_eq!(
        shape("SUM(Sales[[#Totals],[Amount]])").part,
        TablePart::Totals
    );
    // Two specifiers together mean the pair of them.
    assert_eq!(
        shape("SUM(Sales[[#Headers],[#Data],[Amount]])").part,
        TablePart::HeadersData
    );
    assert_eq!(shape("Sales[@Amount]").part, TablePart::ThisRow);
    assert_eq!(
        shape("Sales[[#This Row],[Amount]]").part,
        TablePart::ThisRow
    );

    // A span of columns keeps both ends.
    assert_eq!(
        shape("SUM(Sales[[Q1]:[Q4]])").columns,
        Some(("Q1".into(), Some("Q4".into())))
    );
    // Unqualified: the table the formula sits in.
    assert_eq!(shape("SUM([Amount])").table, None);
    // An apostrophe escapes a bracket inside a column name.
    assert_eq!(
        shape("Sales[[Total '[%']]]").columns,
        Some(("Total [%]".into(), None))
    );

    // A workbook index is not a structured reference; it still reads as one.
    assert!(parse("[1]Sheet1!A1").is_ok());
    // And an unterminated bracket is an error rather than a silent guess.
    assert!(parse("Sales[Amount").is_err());
}

#[test]
fn summaries_of_a_table_in_one_formula() {
    check(&[
        (
            r#"GROUPBY({"b";"a";"b"},{1;2;3},SUM)"#,
            r#"{"a" 2; "b" 4; "Total" 6}"#,
        ),
        (
            r#"GROUPBY({"b";"a";"b"},{1;2;3},LAMBDA(x,MAX(x)),,0,-2)"#,
            r#"{"b" 3; "a" 2}"#,
        ),
        // Headers found in the data and shown on request.
        (
            r#"GROUPBY({"Region";"b";"a";"b"},{"Sales";1;2;3},SUM,3,0)"#,
            r#"{"Region" "Sales"; "a" 2; "b" 4}"#,
        ),
        (
            r#"GROUPBY({"b";"a";"b"},{1;2;3},COUNT,,1,1,{TRUE;TRUE;FALSE})"#,
            r#"{"a" 1; "b" 1; "Total" 2}"#,
        ),
        // Two fields, with a subtotal under each outer group.
        (
            r#"GROUPBY({"x","p";"x","q";"y","p"},{1;2;4},SUM,,2)"#,
            r#"{"x" "p" 1; "x" "q" 2; "x" (blank) 3; "y" "p" 4; "y" (blank) 4; "Total" (blank) 7}"#,
        ),
        (
            r#"PIVOTBY({"a";"a";"b"},{"Q1";"Q2";"Q1"},{1;2;4},SUM)"#,
            r#"{(blank) "Q1" "Q2" "Total"; "a" 1 2 3; "b" 4 (blank) 4; "Total" 5 2 7}"#,
        ),
        // As a file stores it.
        (
            r#"_xlfn.GROUPBY({"b";"a"},{1;2},_xleta.SUM,,0)"#,
            r#"{"a" 2; "b" 1}"#,
        ),
        ("PERCENTOF({1;3},{1;3;4})", "0.5"),
        ("PERCENTOF(1,0)", "#DIV/0!"),
        (
            "TRIMRANGE(A1:D9)",
            "{1 \"x\" #DIV/0! 10; 2 TRUE (blank) 20; 3 (blank) (blank) 30}",
        ),
        (r#"INFO("numfile")"#, "2"),
        (r#"INFO("nonsense")"#, "#VALUE!"),
        (r#"PHONETIC("東京")"#, "\"東京\""),
        (r#"EUROCONVERT(1.2,"DEM","EUR")"#, "0.61"),
        (
            r#"ROUND(EUROCONVERT(1,"FRF","DEM",TRUE,3),12)"#,
            "0.29728616",
        ),
        (r#"EUROCONVERT(1,"FRF","DEM",FALSE,3)"#, "0.3"),
        (
            r#"REGEXEXTRACT("ИНН 7707083893","\d{10}")"#,
            "\"7707083893\"",
        ),
        (r##"REGEXREPLACE("a1b2","\d","#")"##, "\"a#b#\""),
        (r#"REGEXTEST("abc","[")"#, "#VALUE!"),
    ]);
}
