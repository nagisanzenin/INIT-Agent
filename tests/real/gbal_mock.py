"""The scripted model for `tests/real/gbal_proof.sh --mock` ($0): the same owner flow, with the model's side played by
init-mockllm (body/provider.md §3.12). It checks the command forms (initctl goal …, the T0 commit, promote), the Root's
records and the driver, not the model.

usage: python3 -I gbal_mock.py <out.mock.jsonl>
"""
import json, sys

L = [{"v": 1, "kind": "meta", "provider": "deepseek", "model": "deepseek-flash", "name": "gbal-proof-mock", "chunk": 64}]
U = {"input": 12000, "cache_read": 11000, "output": 20}


def call(cmd):
    L.append({"v": 1, "kind": "turn", "reply": {"calls": [{"name": "bash", "input": {"command": cmd}}], "stop": "tool_use", "usage": U}})


def text(t):
    L.append({"v": 1, "kind": "turn", "reply": {"text": t, "stop": "end_turn", "usage": U}})


def change(n, skill, body, card_expect):
    wt = f'"$INIT_HOME/work/wt-g{n}"'
    call(f'cd "$INIT_HOME/body" && git fetch -q origin && git worktree add {wt} -b evo/g{n} origin/main && mkdir -p {wt}/skills/{skill} && '
         f"printf '%s\\n' '---' 'name: {skill}' 'description: {body}' '---' '' '# {skill}' '{body}' > {wt}/skills/{skill}/SKILL.md && "
         f"cd {wt} && git add -A && git commit -q -m 'skill: {skill}' -m \"$(printf 'Why: The owner goal (fewer requests per turn).\\nPrefix-change: yes')\" "
         f"&& git log -1 --format='%H %an <%ae>%n%B'")
    call(f"initctl promote --check evo/g{n}")  # G7: the commit rules first, nothing promoted
    call(f"initctl goal predict sha=$(git -C {wt} rev-parse HEAD) mode=A card=requests_turn {card_expect} note=mock-round-{n}")
    call(f"initctl promote evo/g{n} --wait")


for i in range(5):  # baseline: 2 requests a turn
    call("ls -la; wc -l README.md"); text(f"baseline answer {i}")
call("initctl goal status")
call("initctl goal read card=requests_turn")
change(1, "short-answers", "For short questions about workspace files, answer from one combined shell command.", "expect=better p_permille=700")
text("round 1: predicted, committed, promoted")
for i in range(5):  # after: 1 request a turn (better)
    text(f"after answer {i}")
change(2, "triple-read", "Re-read every file three times before answering.", "expect=worse p_permille=800")
text("round 2: the deliberate regression is live")
for i in range(5):  # after2: 4 requests a turn (cost_turn regresses beyond the guard's 30%)
    call("cat README.md"); call("cat README.md"); call("cat README.md"); text(f"after2 answer {i}")
call("initctl goal status")
call("initctl goal stop reason=met note=mock")
text("stopped")
for i in range(20):
    text(f"spare {i}")
open(sys.argv[1], "w").write("\n".join(json.dumps(x, sort_keys=True) for x in L) + "\n")
