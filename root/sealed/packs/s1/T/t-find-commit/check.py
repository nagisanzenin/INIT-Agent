# t-find-commit: compare answers.txt with the expected answers; check that main is untouched (in a copy).


def parse_answers():
    text = read_text(os.path.join(WS, "answers.txt"))
    if text is None:
        return None
    got = {}
    dup = set()
    for line in text.replace("\r\n", "\n").split("\n"):
        line = line.strip()
        if not line or "=" not in line:
            continue
        k, v = line.split("=", 1)
        k, v = k.strip(), v.strip()
        if k in got:
            dup.add(k)
        got[k] = v
    for k in dup:
        got[k] = None  # a key given twice counts as wrong
    return got


def main():
    want = SPEC["answers"]
    got = parse_answers()

    def answer(key):
        def fn():
            if got is None:
                say("answers.txt missing")
                return False
            v = got.get(key)
            if v is None:
                say("%s: missing" % key)
                return False
            if key == "author":
                return v.lower() == want[key].lower()
            ok = len(v) == 40 and v.lower() == want[key]
            if not ok:
                say("%s: wrong or abbreviated sha" % key)
            return ok
        return fn

    def repo_untouched():
        if got is None:
            return False  # nothing was done
        repo = os.path.join(copy_ws(), "repo")
        code, out = git(["symbolic-ref", "-q", "HEAD"], repo)
        if code != 0 or out.strip() != "refs/heads/main":
            say("HEAD is not the branch main")
            return False
        code, out = git(["rev-parse", "-q", "--verify", "refs/heads/main^{commit}"], repo)
        if code != 0 or out.strip() != SPEC["main_tip"]:
            say("main moved")
            return False
        return True

    run_checks([("introduced", answer("introduced")), ("removed", answer("removed")),
                ("author", answer("author")), ("repo_untouched", repo_untouched)])
