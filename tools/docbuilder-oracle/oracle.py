#!/usr/bin/env python3
"""ONLYOFFICE Document Builder as a third engine.

Formulas, one per line, on stdin; each answer is printed next to its formula,
the same shape `examples/xcheck.rs` prints, so the two outputs diff directly:

    python3 tools/docbuilder-oracle/oracle.py < formulas.txt
    cargo run --release --example xcheck     < formulas.txt

With `--render in.xlsx out.pdf` it draws a workbook instead. That is the one
thing excelize cannot do: it says whether a file we wrote renders at all.

Needs `pip install document-builder`.
"""

import sys

import docbuilder


def render(src, dst):
    b = docbuilder.CDocBuilder()
    if b.OpenFile(src, "") != 0:
        sys.exit(f"cannot open {src}")
    if b.SaveFile(docbuilder.FileTypes.Graphics.PDF, dst) != 0:
        sys.exit(f"cannot render {src}")
    b.CloseFile()


def evaluate(formulas):
    b = docbuilder.CDocBuilder()
    b.CreateFile(docbuilder.FileTypes.Spreadsheet.XLSX)
    sheet = b.GetContext().GetGlobal()["Api"].Call("GetActiveSheet")
    for row, formula in enumerate(formulas, 1):
        sheet.Call("GetRange", f"A{row}").Call("SetValue", formula)
    # Written first, read second: the engine recalculates lazily.
    # `GetValue2` is the stored value; `GetValue` returns numbers as an empty
    # string, and `GetText` returns them formatted.
    for row, formula in enumerate(formulas, 1):
        answer = sheet.Call("GetRange", f"A{row}").Call("GetValue2").ToString()
        print(f"{formula}\t{answer}")
    b.CloseFile()


if __name__ == "__main__":
    if sys.argv[1:2] == ["--render"]:
        render(sys.argv[2], sys.argv[3])
    else:
        evaluate([line.rstrip("\n") for line in sys.stdin if line.strip()])
