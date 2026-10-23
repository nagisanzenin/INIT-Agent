# reference: release-1.1 = v1.0 + the backportable fix commits of main, oldest first
set -eu
export GIT_COMMITTER_DATE="2026-10-02T10:00:00+00:00"
git checkout -q -b release-1.1 v1.0
for c in $(git log --reverse --format=%H v1.0..main); do
  subj=$(git log -1 --format=%s "$c")
  case "$subj" in fix:*) ;; *) continue ;; esac
  case "$subj" in *"[no-backport]"*) continue ;; esac
  released=0
  for f in $(git diff-tree --no-commit-id --name-only -r "$c"); do
    if git cat-file -e "v1.0:$f" 2>/dev/null; then released=1; fi
  done
  [ "$released" = 1 ] || continue
  git cherry-pick "$c" >/dev/null
done
rm -rf .git/logs .git/COMMIT_EDITMSG .git/ORIG_HEAD .git/CHERRY_PICK_HEAD .git/AUTO_MERGE
rm -f .git/index
git read-tree HEAD
