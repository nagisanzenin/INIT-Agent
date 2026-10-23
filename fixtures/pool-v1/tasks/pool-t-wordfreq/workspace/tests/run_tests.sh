#!/bin/sh
# Visible examples for bin/wordfreq: sh tests/run_tests.sh (from the project root)
fail=0
check() { # <name> <expected> <command...>
  name=$1; want=$2; shift 2
  got=$("$@" 2>/dev/null)
  if [ "$got" = "$want" ]; then echo "ok   $name"; else echo "FAIL $name"; echo "  want: $want"; echo "  got:  $got"; fail=1; fi
}
TAB=$(printf '\t')
check top3 "6${TAB}the
3${TAB}cat
3${TAB}mat" bin/wordfreq -n 3 samples/a.txt
check stdin "2${TAB}b
1${TAB}a" sh -c 'printf "b a B" | bin/wordfreq'
exit $fail
