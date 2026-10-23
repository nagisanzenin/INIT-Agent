"""Hidden check items of evo-dbg-sheet (author tool input; `dev.py cases` runs them through the reference).

ITEMS = [(name, [case, ...]), ...]; a case runs `run.py book.csv [edits.txt]` in a scratch dir. Every expected
behaviour follows from workspace/SPEC.md. Inputs differ from the visible examples.
"""


def _col_index(letters):
    n = 0
    for ch in letters:
        n = n * 26 + (ord(ch) - 64)
    return n


def _split(name):
    i = 0
    while name[i].isalpha():
        i += 1
    return int(name[i:]), _col_index(name[:i])


def _q(s):
    if s == "" or not any(c in s for c in ',"\n\r'):
        return s
    return '"' + s.replace('"', '""') + '"'


def csv_text(rows, eol="\n"):
    return "".join(",".join(_q(f) for f in r) + eol for r in rows)


def grid(cells):
    """Rows of a sheet given as {"B3": content, ...}."""
    pos = {_split(k): v for k, v in cells.items()}
    nr = max(r for r, _ in pos)
    nc = max(c for _, c in pos)
    return [[pos.get((r, c), "") for c in range(1, nc + 1)] for r in range(1, nr + 1)]


def case(rows, edits=None):
    files = {"book.csv": csv_text(rows) if isinstance(rows, list) else rows}
    argv = ["book.csv"]
    if edits is not None:
        files["edits.txt"] = edits
        argv.append("edits.txt")
    return {"files": files, "argv": argv}


def cells(**kw):
    return case(grid(kw))


def column(formulas, **extra):
    """Formulas in B1, B2, ... with labels in A; extra cells (e.g. D1="5") are added as given."""
    d = {}
    for i, f in enumerate(formulas, 1):
        d["A%d" % i] = "r%d" % i
        d["B%d" % i] = f
    d.update(extra)
    return grid(d)


ITEMS = []


def item(name, *cases):
    ITEMS.append((name, list(cases)))


# ------------------------------------------------------------------ display and CSV

item("display_numbers",
     case(column(["=1/3", "=10^14+0.5", "=10^14", "=10^15", "=-7/8", "=-0*5", "=0.00012345", "=0.000012345",
                  "=123456789.125", "=1.5E3*2", "=355/113", "=IF(B1>0,B1*3,-B1)", "=-2^0.5*100"])),
     case([["1.50", "+7", "-3", "4.", ".25", "1E3", "2.5e-3", "0012", "-1.0e+2"],
           ["=SUM(A1:I1)", '=ROUND(A2,1)&" ~ "&A2', "=I1/3", "=C1-B1", "", "", "", "", ""]]),
     case(column(["=2^50", "=-(10^15-1)", "=0.1*3", "=1-0.9", "=1/7", "=B5*B5", "=10^-5", "=-1/10^5",
                  "=99999.99999*10", "=2/3*-1"])))

item("csv_input_typing",
     case('"12", 12,\'12,TRUE,false,"=2*3"\r\n"a""b",x"y,"line1\nline2",,"",end\r\n\r\n\' ,=LEN(C2),=A1+1'),
     case(csv_text([[" 12", "1e3", "1e", "--1", "+.5", "TRUE ", "tRuE", "'TRUE", "'", "=1"],
                    ["=COUNT(A1:J1)", "=COUNTA(A1:J1)", "=COUNTIF(A1:J1,TRUE)", "=A1&\"|\"&F1&\"|\"",
                     "=H1=G1", "=LEN(I1)", "=C1&D1", "=E1*4", "", ""]])),
     case("x,1\ny,2\n\n\nz,=B1+B2"))

item("csv_output_quoting",
     case(grid({"A1": '=" lead"', "B1": '="trail "', "C1": '="a"",""b"', "D1": '="x"&","',
                "E1": '="in"&" "&"side"', "A2": "plain", "B2": "'=x", "C2": '=A3&"!"', "A3": "two\nlines",
                "B3": "cr\rinside", "C3": '=IF(LEN(D1)>1,D1&D1,"")', "D3": '=""""'})),
     case([["", "", "", ""]] * 1 + [["=A1", "", "", ""]],
          "set A1  spaced\nset B1 a,b\nset C1 \"q\"\nset D1 '\"\nprint A1:D1\nprint A2\nset A1 ok \nprint A1:A2\n"),
     case(grid({"A1": '=B1&", "&B1', "B1": '=UPPER("x")&" "', "C1": "=LEN(A1)"})))

# ------------------------------------------------------------------ grammar

item("precedence_assoc",
     case(column(["=10-4-3", "=2*3+4*5", "=100/10/5", "=2+3&4", "=1&2+3", "=1+2=3", "=2*3>5=TRUE", "=(1+2)*3",
                  "=-(1+2)*3", '="a"&1+1', "=2&3*2", "=6/2*3", "=6-2+1", "=8-2-1-1", "=B1-B1/2",
                  "=3>2&1"])),
     case(grid({"A1": "20", "A2": "4", "A3": "2", "B1": "=A1-A2-A3", "B2": "=A1/A2/A3", "B3": "=A1-A2*A3",
                "B4": "=A1&A2-A3", "B5": "=A1/A2*A3", "B6": "=(A1-A2)-(A3-A2)", "B7": "=B2*B2-B3",
                "B8": "=A3=A3*1"})))

item("power_unary",
     case(column(["=-3^2", "=(-3)^2", "=2^3^2", "=2^-2", "=2^-1^2", "=4^0.5", "=-4^0.5", "=2*-2^2", "=-2^-2",
                  "=--2^2", "=-2^3", "=-1^2+5", "=3^2^0", "=+2^2", "=1--2^2"])),
     case(grid({"A1": "3", "B1": "=-A1^2", "B2": "=-A1^2+A1*4", "B3": "=1-A1^2", "B4": "=-A1^0.5",
                "B5": "=(-A1)^2", "B6": "=2^-A1", "B7": "=-A1^-1"})),
     case(grid({"A1": "3", "B1": "=-A1^2", "C1": "=10-B1"}), "set A1 -2\nrecalc\nprint A1:C1\nset A1 0.5\nprint B1:C1\n"))

item("comparisons_types",
     case(grid({"A1": "TRUE", "A2": "1", "A3": "'1", "A4": "apple", "A6": "FALSE", "A7": "0",
                "B1": "=A1=A2", "B2": "=A1>A2", "B3": "=A1>99", "B4": "=A2<A3", "B5": "=A3<A1", "B6": "=A4<A1",
                "B7": "=A5=A6", "B8": "=A5=A7", "B9": '=A5=""', "B10": "=A5<A2", "B11": "=A6<A1", "B12": "=A6=A7",
                "B13": "=1<2<3", "B14": "=A7<>A6"})),
     case(column(['="Zebra">"apple"', '="abc"="ABC"', '="a"<"B"', '="b"<"A"', '="10"<"9"', "=10<9",
                  '="abc"<"abcd"', '=""<"a"', '="Same"<>"same"', '="x"<=1'])))

item("coercions",
     case(column(['="3"+1', '=" 4 "*2', '=""+1', "=TRUE+TRUE", "=D9*5", '=-"2"', '=+"x"', '="1e2"/4', '="12abc"+0',
                  "=FALSE*3", '="TRUE"+0', '=1.5&""', '=1/8&"!"', '=(1=1)&""', '=D9&""', "=D1*2", "=D2+1",
                  "=B17*B17"], D1="'42", D2=" 7")),
     case(grid({"A1": "'5", "A2": "TRUE", "B1": "=A1+A2", "B2": "=A1&A2", "B3": "=A1*-1", "B4": "=B1/B1"})))

# ------------------------------------------------------------------ errors

item("error_precedence",
     case(column(["=1/0+NOPE()", "=NOPE()+1/0", '="x"+1/0', '=1/0&"x"', "=(1/0)=(foo)", '="a"*1/0',
                  "=SUM(D1:D3)", "=SUM(E1:F2)", '=SUM("x",1/0)', '=SUM(1/0,"x")', "=CONCAT(1,foo,1/0)",
                  "=IF(1/0,foo,1)", '=AND(TRUE,"x",1/0)', "=MAX(E1:F2,NOPE())", "=B7&B7"],
                 D1="1", D2="=bad", D3="=1/0", E1="2", F1='="x"*1', E2="=2/0", F2="4")),
     case(grid({"A1": "=B1+C1", "B1": "=1/0", "C1": "=zz", "A2": "=C1+B1", "A3": "=A1&A2",
                "A4": "=COUNTA(A1:C1)", "A5": "=COUNT(A1:C1)", "A6": "=A1=A1"})))

item("div_and_domain_errors",
     case(column(["=5/0", "=0/0", "=D9/D9", "=0^-2", "=(-8)^(1/3)", "=(-8)^3", "=0^0", "=10^400", "=-10^400",
                  "=1e300*1e300", "=AVERAGE(D9:D12)", '=AVERAGE("1",D9)', "=1/(1/0)", '=IF(B6=0,"zero",B6)',
                  "=1e308+1e308", "=(-2)^0.5"])))

item("names_parse_errors",
     case(column(["=NOPE(D1:D3)", "=total", "=A0", "=AB12C", "=SUM(1,,2)", "=(1+2", "=1 2", '="x', "=1@2",
                  "=$A", "=", "=NOT(1,2)", "=ROUND()", "=IF(1)", "=#name?", "=sum (1)", "=A1:", "=:A1",
                  "=1+*2", '=Upper("x")', "=B1&B1", "=sum(1)+Sum(2)", "=A1:B", "=SUM(1)2"])))

# ------------------------------------------------------------------ functions

item("sum_average_mixed",
     case(grid({"A1": "10", "A2": "'20", "A3": "TRUE", "A5": "5.5", "A6": "FALSE", "A7": "x",
                "B1": "=SUM(A1:A7)", "B2": "=AVERAGE(A1:A7)", "B3": "=SUM(A1,A2,A3)", "B4": '=SUM(10,"20",TRUE)',
                "B5": "=SUM(A2+0)", "B6": "=AVERAGE(A2,A4)", "B7": "=SUM((A3))", "B8": "=AVERAGE(A1:A7,A3)",
                "C1": "0", "C2": "6", "C3": "-0", "B9": "=AVERAGE(C1:C3)", "B10": "=COUNT(A1:C3)"})),
     case(grid({"A1": "4", "A2": "=A1*2", "A3": "TRUE", "A4": "=A1>3",
                "B1": "=SUM(A1:A4)", "B2": "=AVERAGE(A1:A4)", "B3": "=SUM(A1:A4,A4)", "B4": "=SUM(A3:A4)"})))

item("min_max_count",
     case(grid({"A1": "-5", "A2": "-2", "A3": "TRUE", "A4": "FALSE", "B1": "4", "B2": "7", "B3": "FALSE",
                "C1": "=MAX(A1:A4)", "C2": "=MIN(B1:B3)", "C3": "=COUNT(A1:B3)", "C4": "=COUNTA(A1:B4)",
                "C5": '=COUNT(1,"2",TRUE,D9)', "C6": "=MIN(A3:A4)", "C7": "=MAX(A1:A4,-1)",
                "D1": "0", "D2": "3", "C8": "=MIN(D1:D2)", "C9": "=MAX(A1:A2,D1)"})),
     case(grid({"A1": "3", "A2": "=1/0", "A3": '=""', "A4": "'7", "A5": "=A1>2",
                "B1": "=COUNT(A1:A5)", "B2": "=COUNTA(A1:A5)", "B3": "=COUNTA(A1:A6)", "B4": "=MAX(A3:A5)",
                "B5": "=MIN(A1,A4)", "B6": "=COUNT(A5,A1)", "B7": "=COUNTA(A2,D9)"})))

item("if_lazy",
     case(grid({"A1": "0", "B1": '=IF(A1=0,"n/a",10/A1)', "B2": '=IF(A1<>0,10/A1,"skip")',
                "B3": "=IF(FALSE,NOPE(),3)", "B4": "=IF(TRUE,4,undefined_name)", "B5": "=IF(0,1)",
                "B6": '=IF("TRUE",1,2)', "B7": '=IF("yes",1,2)', "B8": "=IF(D9,1,2)", "B9": "=IF(2,D9)",
                "B10": "=IF(1,A1:A2)", "B11": "=IF(1/0,1,2)", "B12": "=IF(TRUE,,1)", "B13": "=IF(B5,B5,B5)"})),
     case(grid({"C1": "=C1+1", "D1": "=IF(TRUE,5,C1)", "D2": "=IF(FALSE,5,C1)", "D3": "=IF(D1>4,D1*2,0)",
                "D4": "=IF(D2=1,1,2)"})),
     case(grid({"A1": "0", "B1": "=IF(A1=0,0,100/A1)", "C1": "=IF(B1>10,B1-10,B1)"}),
          "set A1 4\nrecalc\nprint B1:C1\nset A1 0\nprint B1:C1\nset A1 -5\nprint B1:C1\nrecalc\n"))

item("and_or_not",
     case(grid({"A1": "TRUE", "A2": "1", "A3": "x", "A5": "=1-1",
                "B1": "=AND(A1:A5)", "B2": "=OR(A1:A5)", "B3": "=AND(A3:A4)", "B4": '=OR("x")',
                "B5": '=AND("true","FALSE")', "B6": "=OR(0,A4)", "B7": "=NOT(A4)", "B8": '=NOT("x")',
                "B9": "=AND(1,1/0)", "B10": "=OR(TRUE,NOPE())", "B11": "=AND(A1,A2,A4)", "B12": "=NOT(-0.5)"})),
     case(grid({"A1": "=1<2", "A2": "=A1", "B1": "=IF(AND(A1,A2),A1,A2)", "B2": "=OR(A1:A2,FALSE)",
                "B3": "=NOT(B1)", "B4": '=AND(A1:A2,"TRUE",2)'})))

item("round_abs",
     case(column(["=ROUND(0.5)", "=ROUND(1.5)", "=ROUND(2.5)", "=ROUND(-0.5)", "=ROUND(-2.5)", "=ROUND(2.675,2)",
                  "=ROUND(1.005,2)", "=ROUND(0.285,2)", "=ROUND(-0.125,2)", "=ROUND(1234.5678,-2)",
                  "=ROUND(1250,-2)", "=ROUND(-1250,-2)", "=ROUND(3.14159,3.9)", "=ROUND(7.5,-0.9)"])),
     case(column(['=ROUND("2.5")', "=ROUND(D9)", "=ROUND(1/3,4)", "=ABS(-2.5)", '=ABS("-3")', "=ABS(TRUE)",
                  "=ROUND(ABS(-2.345),2)", "=ROUND(B7*B7,1)", "=ROUND(-1/0)", '=ROUND(5,"x")', "=ROUND(0.05,1)",
                  "=ROUND(-45,-1)"])))

item("text_functions",
     case(column(["=LEN(1/3)", "=LEN(2^0.5)", "=LEN(TRUE)", "=LEN(D9)", '=LEN("")', '=UPPER("mixed Case 1")',
                  "=UPPER(1/4)", "=UPPER(FALSE)", "=CONCAT(D1:E2)", '=CONCAT(D9,"x",D9)', '=CONCAT(1/0,"x")',
                  "=LEN(1E15)", "=LEN(-0.5)", "=LEN(D1:D2)", "=UPPER(NOPE())", '=LEN(B9)&"/"&B9'],
                 D1="a", E1="b", D2="1", E2="=TRUE")))

item("sumif_countif",
     case(grid({"A1": "5", "A2": "12", "A3": "apple", "A4": "Apple", "A6": "7.5",
                "B1": "1", "B2": "2", "B3": "3", "B4": "4", "B5": "5", "B6": "6",
                "C1": '=COUNTIF(A1:A6,">6")', "C2": '=COUNTIF(A1:A6,"apple")', "C3": '=COUNTIF(A1:A6,"<>apple")',
                "C4": '=COUNTIF(A1:A6,"")', "C5": '=COUNTIF(A1:A6,"<>")', "C6": '=SUMIF(A1:A6,">=7.5",B1:B6)',
                "C7": '=SUMIF(A1:A6,"<10")', "C8": '=SUMIF(A1:A6,"APPLE",B1:B6)', "C9": "=SUMIF(A1:A6,5,B1:B6)",
                "C10": '=COUNTIF(A1:A6,"=5")', "C11": '=COUNTIF(A1:A6,"<b")', "C12": '=SUMIF(A1:A6,">0",B1:B5)',
                "C13": '=COUNTIF(A1:A6,"5")', "C14": "=COUNTIF(A1:A6,A5)"})),
     case(grid({"C1": "TRUE", "C2": "1", "C3": "'1", "C4": "FALSE", "C5": "0",
                "D1": "10", "D2": "20", "D3": "30", "D4": "40", "D5": "50",
                "E1": "=COUNTIF(C1:C5,1)", "E2": "=COUNTIF(C1:C5,TRUE)", "E3": '=COUNTIF(C1:C5,">0")',
                "E4": '=COUNTIF(C1:C5,"<>0")', "E5": '=SUMIF(C1:C5,"<>1",D1:D5)', "E6": '=SUMIF(C1:C5,"<=0",D1:D5)',
                "E7": '=COUNTIF(C1:C5,"true")'})),
     case(grid({"A1": "=1/0", "A2": "3", "A3": "3", "B1": "9", "B2": "=NOPE()", "B3": "4",
                "C1": '=COUNTIF(A1:A3,"<>0")', "C2": "=SUMIF(A1:A3,3,B1:B3)", "C3": '=SUMIF(A2:A3,"<>3",B2:B3)',
                "C4": "=COUNTIF(A1:A3,1/0)", "C5": "=SUMIF(5,5)", "C6": "=SUMIF(A2:A3,3,B3:B4)"})))

# ------------------------------------------------------------------ wide sheets


def wide_row(n, values):
    row = [""] * n
    for k, v in values.items():
        row[_col_index(k) - 1] = v
    return row


item("wide_columns",
     case([wide_row(30, {"Y": "1", "Z": "2", "AA": "3", "AB": "4", "AC": "=AA1*10", "AD": "=SUM(Y1:AB1)"}),
           wide_row(30, {"A": "=AB1", "B": "=AZ1", "C": "=COUNT(A1:AD1)", "D": "=CONCAT(Z1:AA1)",
                         "AA": "=AD1+AC1"})]),
     case([wide_row(55, {"AZ": "52", "BA": "53", "BB": "54", "BC": "=BA1-AZ1"}),
           wide_row(55, {"A": "=SUM(AY1:BB1)", "B": "=BC1*BB1", "Z": "=$BA$1", "AA": "=Z2+1"})]),
     case([wide_row(53, {"Y": "2", "Z": "=Y1*3", "AA": "=Z1+1", "AZ": "=AA1*2", "BA": "=AZ1+Z1"})],
          "recalc\nset Y1 5\nrecalc\nprint Y1:BA1\n"))

item("wide_columns_edits",
     case([wide_row(28, {"Z": "5", "AA": "=Z1*2", "AB": "=AA1+1"})],
          "recalc\nset AB2 =AA1*2\nrecalc\nprint Z1:AC2\nset aa1 7\nrecalc\nprint AA1:AB2\n"),
     case([wide_row(27, {"Y": "1", "Z": "=Y1+1"})],
          "copy Z1 AA1:AC1\nprint Y1:AC1\nrecalc\nset $Y$1 10\nprint AC1\nrecalc\n"))

# ------------------------------------------------------------------ copy

item("copy_anchors",
     case(grid({"A1": "5", "B1": "7", "A2": "1", "B2": "=A1+$A1+A$1+$A$1"}),
          "copy B2 C3\nprint C3\ncopy B2 C2:D3\nprint A1:D3\nrecalc\n"),
     case(grid({"A1": "1", "A2": "2", "A3": "3", "B1": "=SUM(A$1:A1)", "C1": "label"}),
          "copy B1 B2:B3\nprint B1:B3\ncopy C1 C2:C3\ncopy D9 C3\nprint C1:C3\nrecalc\ncopy B3 B2:B3\nprint B2:B3\n"),
     case(grid({"A1": "2", "B1": "=A1*A1", "C1": "=B1+B1"}),
          "copy C1 C2\nset A2 3\nset B2 =A2*A2\nprint A1:C2\nrecalc\n"))

item("copy_ref_errors",
     case(grid({"B2": "=A1+C4", "C4": "4", "A1": "1", "D4": "=SUM(A1:B3)"}),
          "copy B2 A1\nprint A1\ncopy D4 D2\nprint D2\ncopy D4 C3\nprint C3\ncopy A1 E5\nprint E5\nrecalc\n"),
     case(grid({"A1": "1", "B1": "=ZZY1+A1", "C1": "=$ZZZ$1+1"}),
          "copy B1 D1\nprint D1\ncopy C1 E2\nprint E2\ncopy B1 C2\nprint C2\n"))

# ------------------------------------------------------------------ cycles

item("cycles_membership",
     case(grid({"A1": "=C1+1", "B1": "=A1*2", "C1": "=B1", "D1": "=A1*2", "E1": "=IF(FALSE,A1,1)",
                "F1": "=COUNT(A1:C1)", "G1": "=COUNTA(A1:C1)", "A2": "2", "B2": "=A2+1", "C2": "=A2*2",
                "D2": "=B2+C2", "E2": "=D2*D2"}),
          "recalc\nprint\n"),
     case(grid({"A1": "1", "A2": "2", "A5": "=SUM(A1:A5)", "B1": "=MAX(B1:B3)", "C1": "=IF(TRUE,1,C1)",
                "D1": "=A5+1", "D2": "=IF(D1>0,D1,0)", "E1": "=A1+A2", "E2": "=E1+E1"}),
          "recalc\nprint\n"),
     case(grid({"A1": "1", "B1": "=A1+1", "C1": "=B1"}),
          "set A1 =B1*2\nrecalc\nprint A1:C1\nset A1 3\nrecalc\nprint A1:C1\n"))

item("cycle_edits",
     case(grid({"A1": "=C1+1", "B1": "=A1*2", "C1": "=B1", "D1": "10"}),
          "print A1:D1\nset C1 =D1\nrecalc\nprint A1:D1\nset D1 1\nprint A1:C1\n"),
     case(grid({"A2": "=B2", "B2": "=A2", "C2": "=A2+B2"}),
          "set B2 =5*2\nrecalc\nprint A2:C2\n"))

# ------------------------------------------------------------------ recalculation

item("recalc_order_load",
     case(grid({"A1": "=C3+1", "B1": "=A1*2", "C1": "=B2", "B2": "=C2+A1", "C2": "4", "C3": "=C2/2",
                "D1": "=A1+A1"}),
          "recalc\nprint A1:D3\n"),
     case(grid({"A1": "=SUM(B1:B3)", "B1": "=C1", "B2": "=C2", "B3": "1", "C1": "2", "C2": "=C1*2",
                "A3": "=A1+B2*B2", "A2": "=A3", "C3": "=C3"}),
          "recalc\nprint\n"))

item("recalc_incremental",
     case(grid({"A1": "1", "B1": "=A1*2", "C1": "=A1+3", "D1": "=B1+C1", "E1": "=SUM(A1:A3)", "F2": "=E1*E1"}),
          "set A1 2\nrecalc\nprint A1:F2\nset A3 10\nrecalc\nprint E1:F2\nset A3 10\nrecalc\nset F9 1\nrecalc\n"),
     case(grid({"A1": "1", "A2": "2", "B1": "=A1*10", "C1": "=B1+1", "C2": "=C1+C1"}),
          "set B1 =A2*10\nrecalc\nprint B1:C2\nset A2 3\nrecalc\nprint B1:C2\n"),
     case(grid({"A1": "4", "B1": "=A1*2", "C1": "=B1+1", "A2": "6", "B2": "=A2/2"}),
          "set D1 =C1*10\nset D2 =B2-B1\nrecalc\nset A1 5\nrecalc\nprint A1:D2\nset A2 10\nrecalc\nprint D1:D2\n"))

item("recalc_after_replace",
     case(grid({"A1": "1", "A2": "2", "B1": "=A1*2", "C1": "=B1+1"}),
          "set B1 =A2*3\nrecalc\nset A1 100\nrecalc\nset A2 4\nrecalc\nprint A1:C2\n"),
     case(grid({"A1": "1", "A2": "2", "D1": "=A1+A2"}),
          "set D1 =SUM(A1:A3)\nset A1 9\nrecalc\nset D1 =7\nset A2 1\nrecalc\nprint D1\n"))

item("stale_values_after_replace",
     case(grid({"A1": "1", "B1": "=A1", "C1": "=B1", "D1": "=B1*2"}),
          "set C1 =D1+1\nprint A1:D1\nset A1 5\nrecalc\nprint A1:D1\n"),
     case(grid({"A3": "1", "B3": "=A3*2"}),
          "set B3 =7\nset A3 =B3+1\nprint A3:B3\nrecalc\n"))

# ------------------------------------------------------------------ edits and output

item("print_and_used_range",
     case(grid({"A1": "a", "B1": "=A1&A1", "B2": "=B1&B1", "C3": "c", "A3": '=""'}),
          "print B3:A1\nprint D9\nprint\nset C3\nprint\nset A3\nprint\nset A1\nset B1\nset B2\nprint\n"
          "print A1:B1\nrecalc\n"),
     case([wide_row(28, {"Y": "1", "Z": "=Y1+1", "AB": "=Y1*2"}), wide_row(28, {"AA": "=AB1+Y1"})],
          "print Y1:AB2\nprint AB1\nrecalc\ncopy Z1 Z2\nprint Z2:AA2\n"))

item("blank_semantics",
     case(column(["=D9", "=IF(TRUE,D9)", '=D9&"x"', "=D9=0", '=D9=""', "=-D9", "=COUNTA(D9,D1)", "=LEN(D9)",
                  "=SUM(D9)", "=AVERAGE(D9)", "=MIN(D9:D12)", '=B1+B1'], D1="z")),
     case(grid({"A1": "5", "B1": "=A1*2", "C1": "=B1+B1", "D1": '=""', "E1": "=COUNTA(D1)", "F1": '=COUNTIF(D1:D2,"")'}),
          "set A1\nrecalc\nprint A1:F1\nset A1 x\nprint A1:C1\nset A1 \nprint A1:C1\n"))

item("cli_errors",
     dict(case(grid({"A1": "1"}), "print A1\n# note\n\nfrobnicate\nprint A1\n"), stderr=True),
     dict(case(grid({"A1": "1"}), "set A1 2\nprint A1\nset ZZZZ1 5\n"), stderr=True),
     dict(case(grid({"A1": "1"}), "recalc now\n"), stderr=True),
     dict(case(grid({"A1": "1"}), " set A1 5\n"), stderr=True),
     {"files": {"book.csv": 'a,"open\n'}, "argv": ["book.csv"], "stderr": True},
     {"files": {}, "argv": ["missing.csv"], "stderr": True},
     {"files": {}, "argv": [], "stderr": True})


# ------------------------------------------------------------------ large sheets


def ledger(n, seed):
    """A ledger: header, n transactions with a running balance chain, and totals below."""
    import random
    rng = random.Random(seed)
    rows = [["Item", "Amount", "Balance", "Flag"]]
    for r in range(2, n + 2):
        amt = str(rng.choice([rng.randint(-90, 250), rng.randint(1, 40) * 5]))
        rows.append(["t%d" % (r - 1), amt, "=B2" if r == 2 else "=C%d+B%d" % (r - 1, r),
                     "=IF(C%d<0,\"low\",\"\")" % r])
    last = n + 1
    rows.append(["Total", "=SUM(B2:B%d)" % last, "=C%d-B%d" % (last, last + 1), "=COUNTIF(D2:D%d,\"low\")" % last])
    return rows


item("long_chain_ledger",
     case(ledger(1400, 11), "print A1399:D1402\nprint B1\n"),
     case(ledger(1200, 12), "set B2 1000\nprint C1199:D1202\nset B1201 -5000\nprint C1199:D1202\nrecalc\n"))

item("long_cycle",
     case([["=A1200+1", "=A600", "=IF(TRUE,1,A5)"]] + [["=A%d+1" % (r - 1), "", ""] for r in range(2, 1201)],
          "print A1198:C1200\nprint B1:C1\nset A1 5\nprint A1198:A1200\nprint B1:C1\nset A1 =A1200\nprint A1:B1\n"))

item("fill_down_copy",
     case([["1", "=A1*2"], ["=A1+1", ""]],
          "copy A2 A3:A1300\nprint A1298:A1300\ncopy B1 B2:B1300\nprint B1299:B1300\nset A1 10\nprint A1299:B1300\n"))

# ------------------------------------------------------------------ structural edits

item("delete_rows",
     case(grid({"A1": "opening", "B1": "500", "A2": "rent", "B2": "1200", "A3": "food", "B3": "300",
                "A4": "gas", "B4": "80", "A5": "misc", "B5": "45", "A6": "total", "B6": "=SUM(B2:B5)",
                "C6": "=COUNT(B2:B5)", "D6": "=AVERAGE(B3:B5)", "C2": "=B2*2", "C4": "=$B$4+B1"}),
          "delete row 2\nprint\nrecalc\ndelete row 4\nprint\ndelete row 2\nprint A1:D4\n"),
     case(grid({"A1": "1", "A2": "2", "A3": "3", "A4": "4", "B1": "=SUM(A3:A4)", "B2": "=SUM(A3:A3)",
                "B3": "=SUM(A4:A2)", "B4": "=A3+A4"}),
          "delete row 3\nprint\ninsert row 2\nset A2 10\nprint\n"))

item("delete_columns",
     case(grid({"A1": "id", "B1": "Jan", "C1": "Feb", "D1": "Mar", "E1": "total",
                "A2": "101", "B2": "10", "C2": "20", "D2": "30", "E2": "=SUM(B2:D2)",
                "A3": "102", "B3": "5", "C3": "6", "D3": "7", "E3": "=SUM(B3:D3)",
                "A4": "", "B4": "=SUM(B2:B3)", "C4": "=SUM(C2:C3)", "D4": "=SUM(D2:D3)", "E4": "=SUM(B4:D4)"}),
          "delete col B\nprint\nrecalc\ninsert col b\nset B1 Dec\nset B2 1\nset B3 2\nprint A1:E3\ndelete col D\nprint\n"))

item("structural_wide",
     case([wide_row(30, {"W": "100", "X": "1", "Y": "2", "Z": "3", "AA": "4", "AB": "=SUM(X1:AA1)",
                         "AC": "=Z1*AA1"}),
           wide_row(30, {"A": "=AC1+Z1", "B": "=COUNT(Y1:AA1)"})],
          "delete col X\nprint X1:AB1\nprint A2:B2\ninsert col Y\nset Y1 10\nprint W1:AC1\nprint A2:B2\n"))

item("edit_session_long",
     case(ledger(1100, 21),
          "set E1 rate\nset F1 0.5\nset G1 =SUM(B3:B6)\nset E2 =C2*F1\nset E3 =E2+C3\nrecalc\nset B2 10\n"
          "print E2:E3\n"
          "set E2 =B2*F1\nset C2 =B2+1\nprint E2:E3\nrecalc\nset B2 20\nrecalc\nprint C2:E3\n"
          "delete row 3\nprint A1100:D1102\nprint G1\nset F1 2\nprint E2:E3\n"))


item("insert_rows",
     case(grid({"A1": "2", "A2": "3", "A3": "5", "B1": "=SUM(A1:A3)", "B2": "=A2*A2", "B3": "=$A$3+B2",
                "C1": "=SUM(A2:A3)", "C3": "=COUNT(A1:A3)"}),
          "insert row 2\nprint\nrecalc\nset A2 7\nprint B1:C4\ninsert row 1\nprint B2:C5\ninsert row 6\nprint\n"),
     case(grid({"A1": "1", "B1": "2", "C1": "=SUM(A1:B1)", "D1": "=C1*C1", "A2": "=B1+$C$1"}),
          "insert col B\nprint\nset B1 10\nprint\ninsert col A\nprint A1:F2\nrecalc\n"))

item("edit_print_session",
     case(grid({"A1": "Name", "B1": "Score", "A2": "ann", "B2": "71", "A3": "bob", "B3": "84",
                "A4": "cy", "B4": "66", "C2": "=B2-B$5", "C3": "=B3-B$5", "C4": "=B4-B$5",
                "A5": "mean", "B5": "=ROUND(AVERAGE(B2:B4),2)", "C5": "=MAX(C2:C4)-MIN(C2:C4)"}),
          "print\nset B4 90\nrecalc\nprint B4:C5\nset A6 note\nset C6 =C5*C5\nprint\nset A6\nset C6\nprint\n"))
