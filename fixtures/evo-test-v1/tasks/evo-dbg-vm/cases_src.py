"""Hidden items of evo-dbg-vm (author file; dev.py cases turns it into hidden/cases.json via the reference).

Every case runs `run.py main.kite` in a fresh dir; stdout and the exit status must match the reference."""


def P(src):
    return {"files": {"main.kite": src.lstrip("\n")}, "argv": ["main.kite"]}


ITEMS = []


def item(name, *srcs):
    ITEMS.append((name, [P(s) for s in srcs]))


# ------------------------------------------------------------------------------------------- arithmetic

item("arith_assoc_chain", r"""
# running balance with fees and refunds
let balance = 1000;
let fee = 15;
let refund = 40;
print(balance - fee - fee + refund);
print(balance - fee * 3 - refund / 4);
let width = 80;
let margin = 6;
print(width - margin - margin, width - 2 * margin + 1);
""", r"""
fn span(lo, hi) { return hi - lo + 1; }
print(span(3, 9), span(10, 10), span(0, 99));
let total = 0;
for (let i = 1; i <= 5; i += 1) { total = total - i + 2 * i; }
print(total);
print(50 - 20 - 10 - 5, 50 - (20 - (10 - 5)));
""", r"""
print(360 / 6 / 3, 360 / (6 / 3), 100 / 7 * 7, 100 % 7 * 2, 2 * 3 % 4);
print(3 ** 2 ** 2, (3 ** 2) ** 2, -2 ** 4, 2 ** 0, 0 ** 0);
print(10 - 2 ** 3 - 1, 1 + 2 * 3 ** 2 - 4 / 2);
""")

item("arith_division_negative", r"""
print(-7 / 2, -7 % 2, 7 / -2, 7 % -2, -7 / -2, -7 % -2);
print(-9 / 3, -9 % 3, 0 / -5, -1 / 10, -1 % 10);
""", r"""
let ok = true;
for (let a = -12; a <= 12; a += 5) {
  for (let b = -5; b <= 5; b += 3) {
    let q = a / b;
    let r = a % b;
    if (q * b + r != a) { ok = false; }
    print(a, b, q, r);
  }
}
print("identity holds:", ok);
""", r"""
fn average(xs) {
  let sum = 0;
  for (let i = 0; i < len(xs); i += 1) { sum += xs[i]; }
  return sum / len(xs);
}
print(average([-3, -4, -4]), average([5, -12]), average([-1, 0]));
let t = -17;
print("t/5:", t / 5, "t%5:", t % 5);
""")

item("arith_compound_assign", r"""
let x = 100;
x -= 30;
x *= 2;
x /= 3;
x %= 7;
print(x);
let y = -100;
y /= 7;
print(y);
y %= 4;
print(y);
""", r"""
let z = 5;
z -= z - 1 + 1;
print(z);
let l = [7, -7, 9];
l[0] /= -2;
l[1] %= 3;
l[2] -= l[0] - l[1];
print(l);
""", r"""
let s = "ab";
s += "cd";
s += s;
print(s, len(s));
let acc = [1];
acc += [2, 3];
print(acc);
""")

item("arith_bigint", r"""
fn fact(n) {
  let r = 1;
  while (n > 1) {
    r *= n;
    n -= 1;
  }
  return r;
}
print(fact(25));
print(2 ** 100 - 1);
print(fact(30) / fact(28), fact(20) % 1000007);
""", r"""
fn digit_sum(n) {
  let s = 0;
  while (n > 0) {
    s += n % 10;
    n /= 10;
  }
  return s;
}
fn reverse_digits(n) {
  let r = 0;
  while (n > 0) {
    r = r * 10 + n % 10;
    n /= 10;
  }
  return r;
}
print(digit_sum(2 ** 64), reverse_digits(1234567890123), digit_sum(0));
let p = 1;
for (let i = 0; i < 70; i += 1) { p *= 3; }
print(p, p - p / 2 - p / 2);
""")

item("compare_equality", r"""
print(1 == 1, 1 == true, nil == false, "1" == 1, [1, 2] == [1, 2], [1, [2, 3]] == [1, [2, 4]]);
print([] != [nil], "abc" < "abd", "Z" < "a", "apple" > "app", 3 >= 3, -1 < -2);
print(10 - 4 - 3 == 3, 2 * 3 == 6 and 6 / 4 == 1);
fn f() { return 1; }
let g = f;
print(f == g, f == fn() { return 1; }, print == print, nil == nil, true != false);
""", r"""
let words = ["pear", "Apple", "fig", "apple", "banana", "Fig"];
for (let i = 1; i < len(words); i += 1) {
  let w = words[i];
  let j = i - 1;
  while (j >= 0 and words[j] > w) {
    words[j + 1] = words[j];
    j -= 1;
  }
  words[j + 1] = w;
}
print(words);
""", r"""
print(1 < 2, (1 < 2) == true);
print(1 < 2 < 3);
""")

# ----------------------------------------------------------------------------------------- control flow

item("loops_while", r"""
let n = 27;
let steps = 0;
while (n != 1) {
  if (n % 2 == 0) { n /= 2; } else { n = 3 * n + 1; }
  steps += 1;
}
print("collatz steps:", steps);
let i = 0;
let out = [];
while (i < 20) {
  i += 1;
  if (i % 3 == 0) { continue; }
  if (i > 14) { break; }
  push(out, i);
}
print(out);
""", r"""
let row = 1;
while (row <= 4) {
  let line = "";
  let col = 1;
  while (true) {
    if (col > row) { break; }
    line = line + str(row * col) + " ";
    col += 1;
  }
  print(line);
  row += 1;
}
""", r"""
let total = 90;
let used = 25;
let wasted = 10;
let days = 0;
while (total - used - wasted > 0) {
  total -= 20;
  days += 1;
}
print(days, total, total - used - wasted);
""")

item("loops_for_continue", r"""
let primes = [];
for (let n = 2; n < 40; n += 1) {
  let is_prime = true;
  for (let d = 2; d * d <= n; d += 1) {
    if (n % d == 0) {
      is_prime = false;
      break;
    }
  }
  if (not is_prime) { continue; }
  push(primes, n);
}
print(primes);
""", r"""
let s = 0;
for (let i = 0; i < 30; i += 1) {
  if (i % 3 == 0) { continue; }
  if (i % 5 == 0) { continue; }
  s += i;
}
print(s);
for (let i = 10; i > 0; i -= 3) {
  if (i == 7) { continue; }
  print("i =", i);
}
""", r"""
let k = 0;
for (; k < 5;) {
  k += 2;
  if (k == 2) { continue; }
  print("k", k);
}
let count = 0;
for (let j = 0; ; j += 1) {
  if (j >= 4) { break; }
  if (j == 1) { continue; }
  count += 10;
}
print(count);
""")

item("loops_nested_control", r"""
let pairs = [];
for (let a = 1; a <= 4; a += 1) {
  for (let b = 1; b <= 4; b += 1) {
    if (b == a) { continue; }
    if (a + b > 6) { break; }
    push(pairs, [a, b]);
  }
}
print(len(pairs), pairs);
""", r"""
let total = 0;
for (let i = 1; i <= 3; i += 1) {
  let j = 0;
  while (j < 5) {
    j += 1;
    if (j == 2) { continue; }
    total += i * j;
  }
  if (i == 2) { continue; }
  total += 100;
}
print(total);
""", r"""
let grid = [[3, 8, 1], [9, 4, 7], [2, 6, 5]];
let target = 7;
let at = nil;
for (let r = 0; r < len(grid); r += 1) {
  for (let c = 0; c < len(grid[r]); c += 1) {
    if (grid[r][c] != target) { continue; }
    at = [r, c];
    break;
  }
  if (at != nil) { break; }
}
print("found at", at);
""", r"""
let header = [1, 2, 3];
let table = [header];
for (let r = 2; r <= 3; r += 1) {
  let row = [];
  for (let c = 1; c <= 3; c += 1) { push(row, r * c); }
  push(table, row);
}
push(table, header);
print(table);
""")

item("if_else_chains", r"""
fn classify(n) {
  if (n % 15 == 0) { return "FizzBuzz"; }
  else if (n % 3 == 0) { return "Fizz"; }
  else if (n % 5 == 0) { return "Buzz"; }
  else { return str(n); }
}
let line = "";
let n = 1;
while (n <= 20) {
  line = line + classify(n) + " ";
  n += 1;
}
print(line);
""", r"""
fn grade(score, penalty, late) {
  let final = score - penalty - late;
  if (final >= 90) { return "A"; } else if (final >= 80) { return "B"; } else if (final >= 65) { return "C"; }
  return "F";
}
print(grade(95, 0, 0), grade(95, 5, 5), grade(90, 10, 1), grade(70, 5, 0), grade(40, 0, 0));
fn sign(x) {
  if (x > 0) { return 1; } else if (x < 0) { return -1; }
  return 0;
}
print(sign(-5), sign(0), sign(9));
""", r"""
fn describe(user) {
  let name = user[0] or "anonymous";
  let age = user[1];
  if (age == nil) { return name + " (age unknown)"; }
  else if (age < 18) { return name + " (minor)"; }
  return name + " (adult)";
}
print(describe(["ana", 31]));
print(describe([nil, 12]));
print(describe(["bo", nil]));
""", r"""
fn shipping(w) { return w * 2; }
fn quote(w) {
  if (w > 10) { return shipping(w) + 5; }
  else { return shipping(w); }
}
print(quote(3), quote(12));
fn shipping(w) { return w + 100; }
print(quote(3), quote(12));
""")

# ------------------------------------------------------------------------------------------------ logic

item("logic_short_circuit", r"""
let calls = 0;
fn touch(v) {
  calls += 1;
  return v;
}
print(touch(nil) or touch(2), calls);
print(touch(1) or touch(2), calls);
print(touch(false) and touch(3), calls);
print(touch(4) and touch(5), calls);
""", r"""
let cfg = [nil, "dark", nil];
let theme = cfg[0] or cfg[1] or "light";
let font = cfg[2] or "mono";
print(theme, font);
let x = 0;
let y = x or 99;
print(y, "" or "empty", [] or [1], false or nil, nil and 1, 0 and 7);
""", r"""
fn safe_div(a, b) { return b != 0 and a / b; }
print(safe_div(10, 2), safe_div(1, 0));
let xs = [];
print(len(xs) > 0 and xs[0] > 3, len(xs) == 0 or xs[0]);
push(xs, 8);
print(len(xs) > 0 and xs[0] > 3, len(xs) == 0 or xs[0]);
""")

item("logic_truthiness", r"""
fn truth(v) {
  if (v) { return "T"; }
  return "F";
}
print(truth(0), truth(""), truth([]), truth(nil), truth(false), truth("x"), truth(truth));
print(not 0, not nil, not not "a", not (1 == 2));
""", r"""
let flags = [true, false, nil, 0, "", [], "no"];
let n = 0;
for (let i = 0; i < len(flags); i += 1) {
  if (flags[i] or false) { n += 1; }
}
print(n);
let m = 0;
let k = 0;
while (k < len(flags)) {
  if (not flags[k]) { m += 1; }
  k += 1;
}
print(m);
""", r"""
let debug = false;
fn log(msg) {
  if (debug) { print("[debug]", msg); }
  return not debug;
}
print(log("one"));
let debug = true;
print(log("two"));
""")

# -------------------------------------------------------------------------------------------- functions

item("functions_recursion", r"""
fn hanoi(n, from, to, via, moves) {
  if (n == 0) { return moves; }
  moves = hanoi(n - 1, from, via, to, moves);
  push(moves, from + to);
  return hanoi(n - 1, via, to, from, moves);
}
let m = hanoi(3, "A", "C", "B", []);
print(len(m), m);
fn ack(m, n) {
  if (m == 0) { return n + 1; }
  if (n == 0) { return ack(m - 1, 1); }
  return ack(m - 1, ack(m, n - 1));
}
print(ack(2, 3));
""", r"""
fn is_even(n) {
  if (n == 0) { return true; }
  return is_odd(n - 1);
}
fn is_odd(n) {
  if (n == 0) { return false; }
  return is_even(n - 1);
}
print(is_even(10), is_odd(7), is_even(7));
fn power_set_size(items, i) {
  if (i == len(items)) { return 1; }
  return 2 * power_set_size(items, i + 1);
}
print(power_set_size([1, 2, 3, 4, 5], 0));
fn tree_sum(t) {
  fn walk(node) {
    if (type(node) == "int") { return node; }
    let s = 0;
    for (let i = 0; i < len(node); i += 1) { s += walk(node[i]); }
    return s;
  }
  return walk(t);
}
print(tree_sum([1, [2, 3], [[4], 5]]), tree_sum(7));
""", r"""
fn bsearch(xs, x, lo, hi) {
  if (lo > hi) { return -1; }
  let mid = lo + (hi - lo) / 2;
  if (xs[mid] == x) { return mid; }
  if (xs[mid] < x) { return bsearch(xs, x, mid + 1, hi); }
  return bsearch(xs, x, lo, mid - 1);
}
let xs = [-40, -12, -3, 0, 7, 19, 33, 51];
let last = len(xs) - 1;
print(bsearch(xs, 19, 0, last), bsearch(xs, -40, 0, last), bsearch(xs, 8, 0, last), bsearch(xs, 51, 0, last));
""", r"""
fn make_fib() {
  let memo = [0, 1];
  while (len(memo) < 20) { push(memo, memo[len(memo) - 1] + memo[len(memo) - 2]); }
  return fn(n) {
    while (len(memo) <= n) { push(memo, memo[len(memo) - 1] + memo[len(memo) - 2]); }
    return memo[n];
  };
}
let fib = make_fib();
print(fib(10), fib(30), fib(19));
""")

item("functions_calls_arity", r"""
fn add3(a, b, c) { return a + b + c; }
print(add3(1, 2, 3));
let f = add3;
print(f(10, 20, 30), type(f), f);
print(add3(1, 2));
""", r"""
print(len("abc"), len([1, 2]));
let n = len("x", "y");
""", r"""
fn nothing() { }
fn early(x) {
  if (x) { return; }
  return 5;
}
print(nothing(), early(true), early(false));
let r = print("side");
print(r);
let g = fn(a) { return a * 2; };
print(g(21), (fn(x, y) { return x - y; })(10, 3));
""", r"""
fn clamp(v, lo, hi) { return v < lo and lo or v > hi and hi or v; }
print(clamp(-5, 0, 10), clamp(5, 0, 10), clamp(50, 0, 10));
""")

# --------------------------------------------------------------------------------------------- closures

item("closures_counters", r"""
fn make_counter(initial) {
  let n = initial or 0;
  return [fn() { n += 1; return n; }, fn() { return n; }, fn() { n = 0; return n; }];
}
let c = make_counter(nil);
let inc = c[0];
let get = c[1];
let reset = c[2];
inc();
inc();
inc();
print(get());
reset();
inc();
print(get());
let d = make_counter(40);
d[0]();
print(get(), d[1]());
""", r"""
fn outer() {
  let x = 1;
  let show = fn() { return x; };
  x = 50;
  let before = show();
  let set = fn(v) { x = v; };
  set(7);
  return [before, show(), x];
}
print(outer());
""", r"""
fn memoize(f) {
  let cache = [];
  return fn(n) {
    while (len(cache) <= n) { push(cache, nil); }
    if (cache[n] == nil) { cache[n] = f(n); }
    return cache[n];
  };
}
let calls = 0;
let slow_square = fn(n) {
  calls += 1;
  return n * n;
};
let fast = memoize(slow_square);
print(fast(4), fast(4), fast(9), fast(4), calls);
""", r"""
fn make_averager(seed) {
  let total = 0;
  let count = 0;
  for (let i = 0; i < len(seed); i += 1) {
    total += seed[i];
    count += 1;
  }
  return fn(x) {
    total += x;
    count += 1;
    return total / count;
  };
}
let avg = make_averager([10, 20, 30]);
print(avg(40), avg(0), avg(100));
""")

item("closures_loop_capture", r"""
let fs = [];
for (let i = 0; i < 4; i += 1) {
  let sq = i * i;
  push(fs, fn() { return sq; });
}
let out = [];
for (let k = 0; k < len(fs); k += 1) { push(out, fs[k]()); }
print(out);
""", r"""
let handlers = [];
let names = ["save", "open", "quit"];
let i = 0;
while (i < len(names)) {
  let label = names[i];
  let clicks = 0;
  push(handlers, fn() { clicks += 1; return label + ":" + str(clicks); });
  i += 1;
}
print(handlers[0](), handlers[0](), handlers[2](), handlers[1](), handlers[0]());
""", r"""
fn make_multipliers(n) {
  let result = [];
  for (let i = 1; i <= n; i += 1) {
    let factor = i;
    push(result, fn(x) { return x * factor; });
  }
  return result;
}
let ms = make_multipliers(3);
print(ms[0](10), ms[1](10), ms[2](10));
let ms2 = make_multipliers(2);
print(ms2[1](7), ms[0](7));
""")

item("closures_for_variable", r"""
let get_i = [];
let get_j = [];
for (let i = 0; i < 3; i += 1) {
  let j = i;
  push(get_i, fn() { return i; });
  push(get_j, fn() { return j; });
}
print(get_i[0](), get_i[2](), get_j[0](), get_j[2]());
""", r"""
let fns = [];
for (let n = 1; n <= 3; n += 1) {
  let base = n * 100;
  fn describe(x) { return base + x; }
  push(fns, describe);
}
print(fns[0](1), fns[1](2), fns[2](3), fns[0] == fns[1]);
""", r"""
let bump = nil;
for (let i = 0; i < 10; i += 1) {
  if (bump == nil) { bump = fn() { i += 3; }; }
  bump();
  print("i is", i);
}
""")

item("closures_nested_levels", r"""
fn level1() {
  let a = 1;
  fn level2() {
    let b = 10;
    fn level3() {
      a += 1;
      b += 1;
      return a * 100 + b;
    }
    return level3;
  }
  let l3 = level2();
  let first = l3();
  let second = l3();
  return [first, second, a];
}
print(level1());
""", r"""
fn compose(f, g) { return fn(x) { return f(g(x)); }; }
fn curry3(f) { return fn(a) { return fn(b) { return fn(c) { return f(a, b, c); }; }; }; }
let inc = fn(x) { return x + 1; };
let dbl = fn(x) { return x * 2; };
print(compose(inc, dbl)(5), compose(dbl, inc)(5));
let vol = curry3(fn(a, b, c) { return a * b * c - a - b; });
print(vol(2)(3)(4), vol(1)(1)(1));
""", r"""
fn range_iter(lo, hi) {
  let cur = lo;
  return fn() {
    if (cur >= hi) { return nil; }
    let v = cur;
    cur += 1;
    return v;
  };
}
let it = range_iter(3, 7);
let v = it();
let seen = [];
while (v != nil) {
  push(seen, v);
  v = it();
}
print(seen, it());
""", r"""
fn make_logger(prefix) {
  let lines = [];
  let n = 0;
  n += 1;
  push(lines, prefix + " started");
  fn log(msg) {
    n += 1;
    push(lines, prefix + " " + str(n) + ": " + msg);
    return len(lines);
  }
  fn dump() { return lines; }
  return [log, dump];
}
let lg = make_logger("app");
lg[0]("load");
lg[0]("run");
print(lg[1]());
""")

item("closures_objects", r"""
fn make_stack() {
  let items = [];
  fn push_item(v) {
    push(items, v);
    return len(items);
  }
  fn pop_item() { return pop(items); }
  return [push_item, pop_item];
}
let stacks = [];
for (let i = 0; i < 3; i += 1) {
  let s = make_stack();
  push(stacks, s);
  s[0](i * 10);
  s[0](i * 10 + 1);
}
print(stacks[0][1](), stacks[1][1](), stacks[2][1](), stacks[0][1]());
""", r"""
let points = [];
for (let i = 0; i < 3; i += 1) {
  let x = i;
  let y = i * i;
  push(points, [fn() { return [x, y]; }, fn(dx) { x += dx; }]);
}
points[1][1](10);
print(points[0][0](), points[1][0](), points[2][0]());
""", r"""
fn make_buttons(labels) {
  let buttons = [];
  for (let i = 0; i < len(labels); i += 1) {
    let label = labels[i];
    fn render() { return "[" + label + "]"; }
    push(buttons, fn() { return render() + "!"; });
  }
  return buttons;
}
let bs = make_buttons(["ok", "cancel", "help"]);
print(bs[0](), bs[1](), bs[2]());
""", r"""
fn make_account(owner) {
  let balance = 0;
  let history = [];
  balance = 100;
  push(history, "open " + owner);
  fn deposit(x) {
    balance += x;
    push(history, "+" + str(x));
    return balance;
  }
  fn report() { return [owner, balance, history]; }
  return [deposit, report];
}
let acc = make_account("ana");
acc[0](25);
print(acc[1]());
""")

item("callbacks_higher_order", r"""
fn map(xs, f) {
  let out = [];
  for (let i = 0; i < len(xs); i += 1) { push(out, f(xs[i])); }
  return out;
}
fn filter(xs, p) {
  let out = [];
  for (let i = 0; i < len(xs); i += 1) {
    if (p(xs[i])) { push(out, xs[i]); }
  }
  return out;
}
fn reduce(xs, f, acc) {
  for (let i = 0; i < len(xs); i += 1) { acc = f(acc, xs[i]); }
  return acc;
}
let nums = [4, 9, 2, 7, 5];
print(map(nums, fn(x) { return x * 3; }));
print(filter(nums, fn(x) { return x % 2 == 1; }));
print(reduce(nums, fn(a, b) { return a + b; }, 0), reduce(nums, fn(a, b) { if (b > a) { return b; } return a; }, 0));
""", r"""
fn map(xs, f) {
  let out = [];
  for (let i = 0; i < len(xs); i += 1) { push(out, f(xs[i])); }
  return out;
}
let adders = [];
for (let k = 1; k <= 3; k += 1) {
  let step = k * 5;
  push(adders, fn(x) { return x + step; });
}
for (let a = 0; a < len(adders); a += 1) {
  print(map([1, 2], adders[a]));
}
""", r"""
let queue = [];
fn schedule(f) { push(queue, f); }
let log = [];
let tasks = ["load", "parse", "save"];
for (let i = 0; i < len(tasks); i += 1) {
  let name = tasks[i];
  let order = i + 1;
  schedule(fn() { push(log, str(order) + "-" + name); });
}
while (len(queue) > 0) {
  let f = queue[0];
  let rest = [];
  for (let j = 1; j < len(queue); j += 1) { push(rest, queue[j]); }
  queue = rest;
  f();
}
print(log);
""", r"""
fn make_lookup(keys, values) {
  let table = [];
  for (let i = 0; i < len(keys); i += 1) { push(table, [keys[i], values[i]]); }
  return fn(k) {
    for (let j = 0; j < len(table); j += 1) {
      if (table[j][0] == k) { return table[j][1]; }
    }
    return nil;
  };
}
let color = make_lookup(["red", "green", "blue"], [1, 2, 3]);
print(color("green"), color("blue"), color("pink"));
""", r"""
fn make_pipeline(prefix) {
  let count = 0;
  return fn(stage) {
    let label = prefix + "/" + stage;
    return fn(x) {
      count += 1;
      return label + "#" + str(count) + "=" + str(x * 2);
    };
  };
}
let p = make_pipeline("job");
let s1 = p("scale");
let s2 = p("load");
print(s1(5), s2(7), s1(1));
""")

# ------------------------------------------------------------------------------------- lists and strings

item("lists_mutation", r"""
let stack = [];
push(stack, 1);
push(stack, 2);
push(stack, 3);
let top = pop(stack);
print(top, stack, len(stack));
let q = [10, 20, 30, 40];
q[0] += q[-1];
q[-1] -= 5;
q[1] *= 2;
print(q);
let i = 2;
q[i - 1] = q[i] - q[i - 1] - 1;
print(q);
""", r"""
let row = [0, 0, 0];
let board = [row, row];
board[0][1] = 5;
print(board, board[0] == board[1]);
let fresh = [[0, 0], [0, 0]];
fresh[0][1] = 5;
print(fresh);
fn fill(n, v) {
  let l = [];
  for (let i = 0; i < n; i += 1) { push(l, v); }
  return l;
}
let m = fill(3, nil);
m[1] = fill(2, "x");
print(m, len(m[1]));
""", r"""
let xs = [1, 2, 3];
print(xs[-3], xs[2]);
xs[-4] = 0;
""")

item("lists_display", r"""
print([]);
print([[], [[]], [nil, true, false]]);
print(["a\"b", "tab\t", "nl\n", "back\\"]);
print("plain\\text", str(["s"]), str("s"));
fn named() {}
print([print, fn() {}, named, [named]]);
""", r"""
let a = [1];
push(a, a);
print(a);
let b = [a, 2];
push(a, b);
print(a, len(a));
""", r"""
let x = [1, "1", [true, "true"]];
let s = str(x);
print(s, len(s), type(s), type(x[2]));
print(str(nil) + str(false) + str(-12));
""", r"""
let pair = [1, 2];
print([pair, pair], [pair, [pair]]);
let empty = [];
let graph = [empty, empty, [1]];
print(graph);
push(empty, 0);
print(graph);
""")

item("strings_ops", r"""
fn itoa(n) {
  if (n == 0) { return "0"; }
  let neg = n < 0;
  let s = "";
  while (n != 0) {
    let d = n % 10;
    if (d < 0) { d = -d; }
    s = chr(48 + d) + s;
    n /= 10;
  }
  if (neg) { s = "-" + s; }
  return s;
}
print(itoa(0), itoa(7), itoa(-305), itoa(123456), itoa(-9));
""", r"""
let s = "";
for (let i = 0; i < 5; i += 1) { s = s + chr(ord("a") + i); }
print(s, len(s), s[0] + s[len(s) - 1], s < "abd", s == "abcde");
let words = ["kite", "Kite", "KITE"];
print(words[0] == words[1], words[1] < words[0], words[2] < words[1]);
""", r"""
fn reverse(s) {
  let r = "";
  for (let i = len(s) - 1; i >= 0; i -= 1) { r = r + s[i]; }
  return r;
}
fn is_pal(s) { return s == reverse(s); }
print(reverse("stressed"), is_pal("racecar"), is_pal("kite"), reverse(""));
print(int("0") + int("-0") + int("0042"));
""")

# ------------------------------------------------------------------------------------------- algorithms

item("algo_sorting", r"""
fn bubble(xs) {
  let n = len(xs);
  for (let i = 0; i < n; i += 1) {
    for (let j = 0; j < n - i - 1; j += 1) {
      if (xs[j] > xs[j + 1]) {
        let t = xs[j];
        xs[j] = xs[j + 1];
        xs[j + 1] = t;
      }
    }
  }
  return xs;
}
print(bubble([5, -2, 9, 0, -7, 3, 3]));
""", r"""
fn qsort(xs) {
  if (len(xs) <= 1) { return xs; }
  let pivot = xs[0];
  let lo = [];
  let hi = [];
  for (let i = 1; i < len(xs); i += 1) {
    if (xs[i] < pivot) { push(lo, xs[i]); } else { push(hi, xs[i]); }
  }
  return qsort(lo) + [pivot] + qsort(hi);
}
print(qsort([31, -4, 15, 9, 26, -53, 5, 8, 97, 9]));
print(qsort(["delta", "alpha", "charlie", "bravo"]));
""", r"""
fn median(sorted) {
  let n = len(sorted);
  let mid = n / 2;
  if (n % 2 == 1) { return sorted[mid]; }
  return (sorted[mid - 1] + sorted[mid]) / 2;
}
print(median([-9, -4, 1]), median([-9, -4, 1, 6]), median([-10, -5, -2, -1]), median([2, 4]));
""")

item("algo_number_theory", r"""
fn ext_gcd(a, b) {
  if (b == 0) { return [a, 1, 0]; }
  let r = ext_gcd(b, a % b);
  return [r[0], r[2], r[1] - (a / b) * r[2]];
}
let pairs = [[240, 46], [-240, 46], [17, -5], [-9, -6]];
for (let i = 0; i < len(pairs); i += 1) {
  let a = pairs[i][0];
  let b = pairs[i][1];
  let r = ext_gcd(a, b);
  print(a, b, r, a * r[1] + b * r[2] == r[0]);
}
""", r"""
fn digits(n) {
  let out = [];
  while (n != 0) {
    push(out, n % 10);
    n /= 10;
  }
  return out;
}
print(digits(4096), digits(-4096), digits(-7));
""", r"""
let perfect = [];
for (let n = 2; n < 130; n += 1) {
  let s = 1;
  for (let d = 2; d * d <= n; d += 1) {
    if (n % d != 0) { continue; }
    s += d;
    if (d * d != n) { s += n / d; }
  }
  if (s == n) { push(perfect, n); }
}
print(perfect);
""")

item("algo_calendar", r"""
fn split_weeks(days) { return [days / 7, days % 7]; }
let offsets = [10, -10, -7, 0, -1, 23];
for (let i = 0; i < len(offsets); i += 1) { print(offsets[i], split_weeks(offsets[i])); }
""", r"""
fn is_leap(y) { return y % 4 == 0 and (y % 100 != 0 or y % 400 == 0); }
let years = [1900, 2000, 2023, 2024, 2100];
let out = [];
for (let i = 0; i < len(years); i += 1) {
  if (is_leap(years[i])) { push(out, years[i]); }
}
print(out);
fn day_of_year(y, m, d) {
  let lengths = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  if (is_leap(y)) { lengths[1] = 29; }
  let n = d;
  for (let i = 0; i < m - 1; i += 1) { n += lengths[i]; }
  return n;
}
print(day_of_year(2024, 3, 1), day_of_year(2023, 12, 31), day_of_year(2000, 1, 1));
""", r"""
fn clock(mins) {
  let h = mins / 60;
  let m = mins % 60;
  return str(h) + ":" + str(m);
}
print(clock(125), clock(-125), clock(-60), clock(59), clock(-59));
""")

# ---------------------------------------------------------------------------------------- scope, errors

item("scope_shadowing", r"""
let v = 1;
fn f() {
  let v = 2;
  { let v = 3; }
  return v;
}
{
  let v = v + 10;
  print(v, f());
}
print(v);
fn g(v) {
  {
    let v = v * 2;
    return v;
  }
}
print(g(21));
""", r"""
fn total() { return base + bonus(); }
let base = 10;
fn bonus() { return 5; }
print(total());
fn bonus() { return 50; }
print(total());
let base = 1;
print(total());
""", r"""
fn later() { return missing + 1; }
print("start");
print(later());
""", r"""
fn outer() {
  fn peek() { return hidden; }
  let hidden = "local";
  return peek();
}
let hidden = "global";
print(outer());
""")

item("globals_redefinition", r"""
let rate = 10;
fn price(n) { return n * rate; }
print(price(3));
let rate = 12;
print(price(3));
fn price(n) { return n * rate + 1; }
fn total(items) {
  let s = 0;
  for (let i = 0; i < len(items); i += 1) { s += price(items[i]); }
  return s;
}
print(total([1, 2]));
let rate = 0;
print(total([1, 2]));
""", r"""
fn show(x) { print("value:", x); }
show(1);
fn print(a, b) { return nil; }
show(2);
""", r"""
let count = 0;
fn bump() {
  count += 1;
  return count;
}
bump();
bump();
print(count, bump());
""")

item("maps_basics", r"""
let ages = {"ana": 31, "bo": 25};
ages["cy"] = 40;
ages["bo"] += 1;
print(ages, len(ages), ages["bo"], has(ages, "cy"), has(ages, "dee"));
let mixed = {1: "int", true: "bool", "1": "string"};
print(mixed[1], mixed[true], mixed["1"], len(mixed), keys(mixed));
let nested = {"list": [1, {"x": nil}], "empty": {}};
print(nested, type(nested), nested == nested, {} == {});
""", r"""
let m = {"a": 1};
print(m["a"]);
print(m["b"]);
""", r"""
let m = {};
m["ok"] = 1;
m[[1]] = 2;
""", r"""
print(keys({"x": 1, "y": 2}), keys([1]));
""")

item("maps_delete_order", r"""
let stock = {};
let items = ["apple", "pear", "fig", "kiwi", "plum", "lime", "date", "nut", "yam", "oat"];
for (let i = 0; i < len(items); i += 1) { stock[items[i]] = (i + 1) * 10; }
print(del(stock, "pear"), del(stock, "kiwi"), del(stock, "apple"), del(stock, "date"));
let report = [];
for (let i = 0; i < len(items); i += 1) {
  let name = items[i];
  if (has(stock, name)) { push(report, name + "=" + str(stock[name])); } else { push(report, name + "=gone"); }
}
print(report);
stock["pear"] = 5;
stock["fig"] += 1;
stock["oat"] = 0;
print(stock, len(stock));
let extra = ["rye", "bean", "corn", "leek", "okra", "taro", "sago", "kale"];
for (let i = 0; i < len(extra); i += 1) { stock[extra[i]] = i; }
print(stock["lime"], stock["nut"], stock["kale"], len(stock), keys(stock));
""", r"""
let seen = {};
let ids = [3, 11, 19, 27, 35, 4, 12];
for (let i = 0; i < len(ids); i += 1) { seen[ids[i]] = i; }
del(seen, 3);
del(seen, 19);
print(has(seen, 11), has(seen, 27), has(seen, 35), seen[12]);
seen[11] = 100;
seen[3] = -1;
print(seen, len(seen), keys(seen));
""")

item("maps_word_freq", r"""
let text = "the quick brown fox jumps over the lazy dog the dog barks and the fox runs away from the dog while the quick cat sleeps";
let words = [];
let w = "";
for (let i = 0; i < len(text); i += 1) {
  let c = text[i];
  if (c == " ") {
    if (len(w) > 0) { push(words, w); }
    w = "";
  } else {
    w = w + c;
  }
}
if (len(w) > 0) { push(words, w); }
let freq = {};
for (let i = 0; i < len(words); i += 1) {
  let k = words[i];
  if (has(freq, k)) { freq[k] += 1; } else { freq[k] = 1; }
}
print(len(words), len(freq));
let ks = keys(freq);
for (let i = 0; i < len(ks); i += 1) {
  if (freq[ks[i]] == 1) { del(freq, ks[i]); }
}
print(freq);
let more = ["owl", "bat", "elk", "emu", "yak", "ant", "bee", "cod", "the", "dog", "gnu", "hen", "jay"];
for (let i = 0; i < len(more); i += 1) {
  if (has(freq, more[i])) { freq[more[i]] += 1; } else { freq[more[i]] = 1; }
}
print(freq, len(freq));
""")

item("maps_lru_cache", r"""
fn make_cache(capacity) {
  let data = {};
  let log = [];
  fn get(k) {
    if (not has(data, k)) {
      push(log, "miss " + str(k));
      return nil;
    }
    let v = del(data, k);
    data[k] = v;
    push(log, "hit " + str(k));
    return v;
  }
  fn put(k, v) {
    if (has(data, k)) { del(data, k); }
    else if (len(data) >= capacity) {
      let oldest = keys(data)[0];
      del(data, oldest);
      push(log, "evict " + str(oldest));
    }
    data[k] = v;
  }
  fn state() { return keys(data); }
  return [get, put, state, log];
}
let c = make_cache(3);
let get = c[0];
let put = c[1];
put(1, "a");
put(2, "b");
put(3, "c");
get(1);
put(4, "d");
get(2);
get(3);
put(5, "e");
put(1, "z");
get(4);
get(1);
for (let i = 10; i < 30; i += 1) {
  put(i, i * i);
  if (i % 3 == 0) { get(i - 1); }
}
print(c[2]());
print(len(c[3]), c[3][len(c[3]) - 1]);
print(get(29), get(27), get(10));
""", r"""
let sessions = {};
let users = ["kim", "lee", "max", "ned", "oli", "pat", "quinn", "ray", "sam", "tia"];
for (let round = 0; round < 3; round += 1) {
  for (let i = 0; i < len(users); i += 1) {
    let u = users[i];
    if ((i + round) % 3 == 0 and has(sessions, u)) { del(sessions, u); }
    else { sessions[u] = round * 10 + i; }
  }
}
let active = [];
for (let i = 0; i < len(users); i += 1) {
  if (has(sessions, users[i])) { push(active, users[i] + ":" + str(sessions[users[i]])); }
}
print(active, len(sessions));
""")

item("maps_graph_search", r"""
let edges = [["a", "b"], ["a", "c"], ["b", "d"], ["c", "d"], ["d", "e"], ["e", "f"], ["c", "f"], ["f", "g"], ["b", "h"], ["h", "g"]];
let adj = {};
for (let i = 0; i < len(edges); i += 1) {
  let u = edges[i][0];
  let v = edges[i][1];
  if (not has(adj, u)) { adj[u] = []; }
  if (not has(adj, v)) { adj[v] = []; }
  push(adj[u], v);
  push(adj[v], u);
}
fn bfs(start, blocked) {
  let dist = {};
  dist[start] = 0;
  let queue = [start];
  let head = 0;
  while (head < len(queue)) {
    let u = queue[head];
    head += 1;
    let ns = adj[u];
    for (let i = 0; i < len(ns); i += 1) {
      let v = ns[i];
      if (has(blocked, v) or has(dist, v)) { continue; }
      dist[v] = dist[u] + 1;
      push(queue, v);
    }
  }
  return dist;
}
print(bfs("a", {}));
let blocked = {"x": true, "d": true, "y": true, "c": true};
del(blocked, "x");
del(blocked, "c");
print(bfs("a", blocked));
""", r"""
let parent = {};
fn find(x) {
  if (not has(parent, x)) { parent[x] = x; }
  if (parent[x] == x) { return x; }
  let root = find(parent[x]);
  parent[x] = root;
  return root;
}
fn union(a, b) {
  let ra = find(a);
  let rb = find(b);
  if (ra != rb) { parent[ra] = rb; }
}
let pairs = [[1, 9], [17, 25], [9, 25], [2, 10], [33, 41], [3, 11]];
for (let i = 0; i < len(pairs); i += 1) { union(pairs[i][0], pairs[i][1]); }
del(parent, 1);
del(parent, 17);
print(find(25), find(9), find(33), find(41), find(1), len(parent));
""")

item("errors_runtime_lines", r"""
let readings = [4, -1, 6, 0, 3];
let inverse = [];
for (let i = 0; i < len(readings); i += 1) {
  if (readings[i] < 0) { continue; }
  push(inverse, 120 / readings[i]);
  print(inverse);
}
""", r"""
/* configuration
   block */
let rate = 4;
# compute
let parts = [
  10,
  20
];
print(parts[0] / rate);
print(parts[1]
  /
  (rate - 4));
""", r"""
let config = nil;
fn load() {
  return config(1);
}
print("loading");
load();
""", r"""
fn down(n) {
  if (n == 0) { return 0; }
  return 1 + down(n - 1);
}
print(down(199));
print(down(200));
""")

item("errors_types", r"""
print("n=" + 5);
""", r"""
let items = [1, 2];
print(items - 1);
""", r"""
print(-[1]);
""", r"""
let s = "abc";
print(s[1], s["0"]);
""", r"""
print(5[0]);
""", r"""
push("x", 1);
""", r"""
print(ord("ab"));
""", r"""
print(int(12));
""", r"""
print("x" < 1);
""")

item("errors_syntax", r"""
print(1);
let = 5;
""", r"""
let a = "unterminated;
print(a);
""", r"""
while (true) {
  print(1);
}
break;
""", r"""
let x = 1;
let x = 2;
print(x);
""", r"""
fn f(a, b, a) {
  return a;
}
""", r"""
{
  let y = 1;
  let y = 2;
}
""", r"""
print("ok");
print(5 = 3);
""", r"""
let s = "bad \q escape";
""", r"""
let n = 12abc;
""", r"""
print(1);
/* never
closed
""", r"""
while (true) {
  fn h() { break; }
}
""", r"""
print("top");
return;
""")
