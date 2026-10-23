# t-merge-rules: inspect the merge commit and the committed files in a private copy of the repository.

MARKERS = ("<<<<<<<", ">>>>>>>", "|||||||")


def main():
    ws = copy_ws()
    repo = os.path.join(ws, "repo")
    cache = {}

    def parents():
        if "p" not in cache:
            code, out = git(["rev-list", "--parents", "-n", "1", "HEAD", "--"], repo)
            cache["p"] = out.split() if code == 0 else []
        return cache["p"]

    def merge_commit():
        code, out = git(["symbolic-ref", "-q", "HEAD"], repo)
        if code != 0 or out.strip() != "refs/heads/main":
            say("HEAD is not the branch main: %r" % out.strip())
            return False
        p = parents()
        if p[1:] != [SPEC["main_tip"], SPEC["feature_tip"]]:
            say("HEAD parents: %s" % " ".join(p[1:]))
            return False
        code, out = git(["rev-parse", "-q", "--verify", SPEC["feature_ref"] + "^{commit}"], repo)
        if code != 0 or out.strip() != SPEC["feature_tip"]:
            say("the feature branch moved")
            return False
        return True

    def file_check(f):
        def fn():
            code, out = git(["cat-file", "blob", "HEAD:" + f["path"]], repo)
            if code != 0:
                say("%s: not in HEAD" % f["path"])
                return False
            ok = norm_lines(out) == norm_lines(f["want"])
            if not ok:
                say("%s: committed content differs from the rules" % f["path"])
            return ok
        return fn

    def worktree_clean():
        p = parents()
        if not p or p[0] == SPEC["main_tip"]:
            say("nothing was committed")
            return False
        if os.path.lexists(os.path.join(repo, ".git", "MERGE_HEAD")):
            say("a merge is still in progress")
            return False
        code, out = git(["status", "--porcelain", "--untracked-files=no"], repo)
        if code != 0 or out.strip():
            say("uncommitted changes: %s" % out.strip()[:200])
            return False
        code, out = git(["ls-files", "-u"], repo)
        if code != 0 or out.strip():
            return False
        code, out = git(["ls-files", "-z"], repo)
        if code != 0:
            return False
        for rel in out.split("\0"):
            if not rel:
                continue
            text = read_text(os.path.join(repo, rel))
            if text is None:
                continue
            for line in text.split("\n"):
                if line.startswith(MARKERS):
                    say("conflict marker left in %s" % rel)
                    return False
        return True

    checks = [("merge_commit", merge_commit)]
    checks += [(f["name"], file_check(f)) for f in SPEC["files"]]
    checks.append(("worktree_clean", worktree_clean))
    run_checks(checks)
