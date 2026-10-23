# t-bisect-skip: compare first_bad.txt with the expected sha; check the repository state in a private copy.

BISECT_FILES = ("BISECT_LOG", "BISECT_START", "BISECT_TERMS", "BISECT_EXPECTED_REV", "BISECT_ANCESTORS_OK",
                "BISECT_NAMES", "BISECT_RUN", "BISECT_HEAD")


def main():
    text = read_text(os.path.join(WS, "first_bad.txt"))
    shas = [ln.strip().lower() for ln in (text or "").split("\n") if ln.strip()]
    answered = len(shas) == 1 and len(shas[0]) == 40 and all(c in "0123456789abcdef" for c in shas[0])
    cache = {}

    def repo():
        if "r" not in cache:
            cache["r"] = os.path.join(copy_ws(), "repo")
        return cache["r"]

    def answer():
        if not answered:
            say("first_bad.txt must hold exactly one full 40-hex sha")
            return False
        return shas[0] == SPEC["first_bad"]

    def head_restored():
        if not answered:
            return False  # nothing was done
        code, out = git(["symbolic-ref", "-q", "HEAD"], repo())
        if code != 0 or out.strip() != "refs/heads/main":
            say("HEAD is not the branch main")
            return False
        code, out = git(["rev-parse", "-q", "--verify", "refs/heads/main^{commit}"], repo())
        if code != 0 or out.strip() != SPEC["main_tip"]:
            say("main moved")
            return False
        return True

    def clean_tree():
        if not answered:
            return False
        code, out = git(["status", "--porcelain", "--untracked-files=all"], repo())
        if code != 0 or out.strip():
            say("work tree not clean: %s" % out.strip()[:200])
            return False
        return True

    def no_bisect_state():
        if not answered:
            return False
        gd = os.path.join(repo(), ".git")
        left = [f for f in BISECT_FILES if os.path.lexists(os.path.join(gd, f))]
        if left:
            say("bisect state left: %s" % left)
            return False
        code, out = git(["for-each-ref", "refs/bisect/"], repo())
        if code != 0 or out.strip():
            say("refs/bisect/* left")
            return False
        return True

    run_checks([("answer", answer), ("head_restored", head_restored), ("clean_tree", clean_tree),
                ("no_bisect_state", no_bisect_state)])
