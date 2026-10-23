#!/bin/sh
fail=0
[ "$(sh calc.sh add 2 3)" = 5 ] || { echo "FAIL add 2 3"; fail=1; }
[ "$(sh calc.sh sub 7 2)" = 5 ] || { echo "FAIL sub 7 2"; fail=1; }
[ $fail = 0 ] && echo "PASS"
exit $fail
