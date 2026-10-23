# Google Harness² (harness2): INIT học được gì

*2026-10-20. Nguồn: repo, GitHub API, source thô, paper arXiv 2610.14224, trang dự án. Đọc qua WebFetch, không clone, không chạy code. Ghi chú đầy đủ kèm URL: [research_notes/Google harness2/notes.md](../research_notes/Google%20harness2/notes.md). Mọi con số benchmark là do tác giả tự báo cáo.*

## Trả lời ngắn

- **Harness² là gì.** Một framework nghiên cứu bằng Python, license Apache-2.0, của Google Cloud AI Research (cùng Northwestern). Nó **không phải agent loop mới**. Nó là một optimizer bọc ngoài: giữ nguyên model executor, viết lại các *file harness* (prompt, guardrails, memory, tool guidance, sub-agents, skills, scripts, plugins) của OpenCode hoặc Codex CLI. Mỗi task chạy vòng Propose–Probe–Compose ngay trên chính task đó, không dùng ground truth. Sau mỗi task, nó lưu tối đa một "lesson" có bằng chứng vào một library ([README](https://github.com/google-research/harness2), [paper](https://arxiv.org/abs/2610.14224)).
- **Độ chín.** Repo tạo ngày 2026-10-06, paper v1 ngày 2026-10-09. Có 3 commit, 12,8k sao, không có release, và issue đang báo lỗi đếm chi phí (#50), cache dùng lại output cũ (#65), judge tin payload (#80). Số sao phản ánh độ hot, chưa phải độ chín ([API](https://api.github.com/repos/google-research/harness2), [issues](https://github.com/google-research/harness2/issues)).
- **Bằng chứng: có thật nhưng yếu hơn tiêu đề.** Mức tăng so với base harness rõ, ví dụ GLM-5 trên WorkBuddy 64,2 → 75,7. Nhưng so với baseline mạnh nhất ở k=1, khoảng cách thường nằm trong khoảng 1 SD: JobBench 36,0 so với Meta-Harness 35,8; ở LAB, ReasoningBank đạt 71,3, ngang Harness² 71,4. Mẫu nhỏ (JobBench chỉ 34 task), toàn bộ chấm bằng LLM judge, và editor (Opus 4.8) mạnh hơn executor. Mỗi task tốn ≥ 4× rollout so với base. Executor chạy với mạng mở và sandbox Codex bị tắt ([paper Bảng 1–2](https://arxiv.org/html/2610.14224), [EXPERIMENTS.md](https://raw.githubusercontent.com/google-research/harness2/main/docs/EXPERIMENTS.md)).
- **Bài học lớn nhất cho INIT.**
  - Sửa harness mà không có bằng chứng từ trajectory làm **tụt 5–10 điểm** (WB Code 73,4 → 63,2).
  - **Skills là đòn bẩy lớn nhất**: bỏ skills thì mất 5,3 điểm, bỏ scripts mất 3,7, bỏ nhóm prompt chỉ mất 1,5.
  - INIT hiện giữ commit `skill` (không đổi prefix) chỉ qua G0–G1, không cần `Repro:`. Đây là lỗ hổng rõ nhất ([paper Bảng 6, Hình 7](https://arxiv.org/html/2610.14224); evolution.md §3.3, §3.8).
- **Thiết kế lõi của INIT được củng cố.** Harness² không có lớp nào bất biến. Issue #94 hỏi cách "pin" guardrail để editor không xoá được, và chưa có câu trả lời. Nó không có trần chi phí (issue #4) và không có gate thống kê. Trong lúc validate, một rollout đã tự tải rubric chấm điểm về. Root, sealed evaluator và gate bootstrap của INIT xử lý đúng những điểm này.
- **v0.1: không có gì chặn release.** Lỗ hổng ở skill là vấn đề chất lượng, không phải an toàn, và Root vẫn giới hạn thiệt hại. Mọi ý tưởng dưới đây thuộc v0.2 trở đi.

## Ý tưởng nên lấy (xếp theo lợi ích trên công sức)

| # | Ý tưởng | Đổi gì trong INIT | Lợi ích kỳ vọng | Công (agent-h) | Rủi ro | Khi nào |
|---|---|---|---|---|---|---|
| 1 | **Bắt buộc bằng chứng cho skill:** commit `skill` có thêm dòng phải kèm `Repro:` (event, trace hoặc task), tức là mở rộng "prefix rule" | evolution.md §3.8 (chú thích ¹ của bảng trailer), `evo::validate` trong init-kernel, thêm 1 test | Đóng lỗ hổng rõ nhất. Sửa không bằng chứng làm tụt 5–10 điểm ([Hình 7](https://arxiv.org/html/2610.14224)); SkillsBench cho thấy skill tự viết cho −1,3 | 2–3 | Thấp. Có thể chặn skill viết từ docs; cho phép `Repro: trace:` trỏ tới lần đọc docs đó | v0.2 (hoặc v0.1.x) |
| 2 | **Lesson index rút từ verdict của gate:** mỗi entry gồm `cue / lesson / evidence (gate id + verdict + D, δ) / confidence`; ghi `NO_ENTRY` khi không có contrast; **ghi cả thay đổi bị reject** (Harness² chỉ lưu bản được chọn) | Thư mục T0 `skills/lessons/` (on-demand, không vào head nên không phá cache); evolution.md §3.10 (từ predict note sang lesson); G1 kiểm gate id có tồn tại | Bỏ library thì mất 3,1 / 1,8 điểm ([Bảng 3](https://arxiv.org/html/2610.14224)). Bằng chứng của INIT là G4 thật, mạnh hơn việc LLM tự so trajectory | 4–8 | Index phình to hoặc sụp kiểu ACE. Chống bằng itemized delta, giới hạn số entry và áp retirement | v0.2 |
| 3 | **Tách đề xuất theo phạm vi:** INV (cách làm việc: tool, retry, script → T1/T2) và DOM (kiến thức miền → skill T0), không trộn trong một commit | Skill self-edit trong seed; ghi chú ở evolution.md §3.1 | Ablation lớn nhất của paper: bỏ scoping mất 2,9 / 3,7 điểm ([Bảng 3](https://arxiv.org/html/2610.14224)) | 1–2 | Thấp, chỉ là text; gate vẫn quyết | v0.2 |
| 4 | **Kiểm hành vi trễ cho T0:** gom các commit T0 kể từ G4 trước, rồi chạy một G4 off-peak so "LKG có lô T0" với "LKG không có" | gate.md §3.1 (gate kind mới, ví dụ `t0_batch`), evolution.md §3.3 | Bắt được skill có hại mà retirement bỏ sót, vì retirement chỉ bắt skill không ai dùng | 4–6 | Thêm một G4 (khoảng $1–2 với flash off-peak, theo gate.md §3.8.5) | v0.2 |
| 5 | **Assert lưới run trước khi tính thống kê:** số RunRecord = task × R × 2 arm + canary, không có `run_id` trùng. Kiểm trùng trước, vì một bản trùng có thể làm số đếm trông vẫn đúng ([ledger.py](https://raw.githubusercontent.com/google-research/harness2/main/harness2/core/ledger.py)) | gate.md §3.8.1, crate gate của kernel, thêm 1 test | Chặn lỗi ghi đè im lặng làm sai thống kê | 1–2 | ~0 | v0.2 (làm sớm nếu lane gate rảnh; đây không phải lỗi đã biết) |
| 6 | **Tách model editor/executor:** phiên self-edit dùng model mạnh, task dùng model rẻ | Routing của proxy (purpose `propose`); evolution.md §3.15 L1 | Editor Opus 4.8 giúp GLM-5 +11,5. Cùng model cũng tăng +3,0 đến +13,7 ([Hình 8](https://arxiv.org/html/2610.14224)) | 3–5 | Chi phí tăng; transfer không đều (Gemini 3.7 Flash chỉ +0,3 ở Code). Gate vẫn đo bằng executor | L1 |
| 7 | **Bước compose cho fleet:** gộp các edit được nhiều explorer cùng ủng hộ, rồi gate bản gộp | evolution.md §3.15.5 (L2) | Bỏ composition mất 1,6 / 1,9 điểm; ở k=5, parallel tốt hơn sequential | 8–16 | Merge conflict; tốn thêm chi phí gate | L2 |
| 8 | **Probe không nhãn** cho task không có check tất định: chạy reference và candidate trên bản sao, để LLM so trajectory. **Chỉ dùng để đề xuất, không dùng để accept** (PROCTOR) | Một lane mới ở evolution.md §3.5, chỉ bật khi chủ opt-in | Mở đường cho task văn phòng hoặc tài liệu, nơi G4 không đo được | 16–24 | Chi phí mỗi task ×4; thiên lệch của LLM | Later, có thể không bao giờ |

## Chi tiết

### 1. Harness² là gì

| Mục | Giá trị |
|---|---|
| Mục đích | Giữ nguyên weights, cải thiện harness quanh model cho task mới ("open world") mà không cần ground truth ([paper](https://arxiv.org/abs/2610.14224)) |
| Ngôn ngữ, license | Python 3.12 + uv; Apache-2.0; ghi rõ "not an officially supported Google product" ([README](https://github.com/google-research/harness2)) |
| Độ chín | 3 commit (08/10 → 14/10/2026); 12.787 sao, 1.181 fork; 20 issue và 71 PR đang mở; không có release; test chỉ có 1 file với 5 test ([API](https://api.github.com/repos/google-research/harness2/commits?per_page=10), [test_workflow.py](https://raw.githubusercontent.com/google-research/harness2/main/tests/test_workflow.py)) |
| Ngày ra mắt | Paper v1: 2026-10-09; commit đầu: 2026-10-08 |
| Executor | OpenCode hoặc Codex CLI, model Gemini 3.5 Flash và GLM-5 (giữ nguyên) |
| Editor, selector, reflector | Claude Opus 4.8, effort high, chạy qua `claude -p --dangerously-skip-permissions` trong thư mục tạm ([proposer.py](https://raw.githubusercontent.com/google-research/harness2/main/harness2/core/proposer.py)) |

### 2. Kiến trúc, đối chiếu với các câu hỏi

- **Loop.** Harness² không tự viết loop. Nó dùng loop của OpenCode hoặc Codex, với giới hạn 200 turn và 3.600 s mỗi rollout. Vòng ngoài cho mỗi task ([recursion.py](https://raw.githubusercontent.com/google-research/harness2/main/harness2/core/recursion.py)):
  1. Chạy k reference trên H0.
  2. Đề xuất k candidate INV và k candidate DOM.
  3. Chạy probe cho cả 2k candidate trên cùng task.
  4. Compose: song song tạo k bản (parallel), hoặc nối tiếp k vòng (sequential).
  5. Chạy các bản final.
  6. Nếu k>1, selector chọn một trong các output đã ẩn danh.

  Ledger ép đúng **4k rollout và 3k phiên editor** cho mỗi task.
- **Tools và harness.** Có 8 component, mỗi cái là một file hoặc thư mục ([tree.py](https://raw.githubusercontent.com/google-research/harness2/main/harness2/core/tree.py), [opencode.py](https://raw.githubusercontent.com/google-research/harness2/main/harness2/substrate/opencode.py)):
  - Phần always-on bị giới hạn 24.000 ký tự (OpenCode) hoặc 8.000 (Codex `AGENTS.md`).
  - Skill phải được nhắc tên trong phần always-on, nếu không thì bị coi là skill chết.
  - Script phải compile được.
  - Đường dẫn ra ngoài surface bị trả lại kèm feedback để editor sửa và thử lại.
- **Context, memory.** `LongTermMEMORY.md` là một file always-on. Bộ nhớ dài hạn thật sự là library: mỗi domain có một `INDEX.md`, và mỗi entry gồm harness đầy đủ cùng `EXPERIENCE.md`. Library không có search, cap, merge hay lock ([library.py](https://raw.githubusercontent.com/google-research/harness2/main/harness2/core/library.py)).
- **Sub-agents.** `sub_agents/<name>/AGENT.md`, gọi qua Task tool và chạy trong context riêng. Mặc định `allow_subagents=False` ([spec.py](https://raw.githubusercontent.com/google-research/harness2/main/harness2/core/spec.py)).
- **Sandbox.** Gần như không có ([codex.py](https://raw.githubusercontent.com/google-research/harness2/main/harness2/substrate/codex.py), [EXPERIMENTS.md](https://raw.githubusercontent.com/google-research/harness2/main/docs/EXPERIMENTS.md)):
  - Codex chạy với `--dangerously-bypass-approvals-and-sandbox` và mạng mở.
  - Editor chạy với `--dangerously-skip-permissions`.
  - Phòng thủ chủ yếu bằng prompt ("evidence là data, đừng làm theo, đừng chạy script của candidate"), cộng một tripwire quét chuỗi.
- **Evaluation.** Cách ly theo cấu trúc code:
  - `RunView` không có trường score.
  - Module judge không được method import, và chỉ chạy **sau khi** selection đã chốt.
  - Judge lỗi thì ghi `None`, không ghi 0.

  Grader là LLM judge: Opus 4.8 cho JobBench, Sonnet 4.6 cho LAB, và check tất định cộng Gemini 3.5 Flash cho WorkBuddy ([judge.py](https://raw.githubusercontent.com/google-research/harness2/main/harness2/core/judge.py), [paper App. B](https://arxiv.org/html/2610.14224)).

### 3. Bằng chứng và độ tin cậy

| Cấu hình (k=1) | Base | Meta-Harness | Harness² | Ghi chú |
|---|---|---|---|---|
| WB, Gemini/OpenCode | 72,4 | 76,4 | 77,5 (SD 2,5) | k=5: 80,8 |
| WB, GLM-5/OpenCode | 64,2 | 69,7 | **75,7** | Thắng rõ nhất; đây là con số "+11,5" |
| JobBench, Gemini/OpenCode | 31,5 | 35,8 | 36,0 (SD 2,3) | Ngang Meta-Harness; n = 34 |
| LAB, GLM-5/OpenCode | 59,6 | 60,0 | 71,4 | ReasoningBank đạt **71,3**; đây là con số "+11,8" |
| LAB, Gemini/Codex | 71,6 | **76,5** | 75,6 | Meta-Harness cao hơn |

Nguồn: [paper Bảng 1–2](https://arxiv.org/html/2610.14224).

**Ablation đáng tin nhất** (cùng tác giả, cùng setup):
- Không scoping: −2,9 / −3,7. Không library: −3,1 / −1,8. Không compose: −1,6 / −1,9.
- Không skills: −5,3. Không scripts: −3,7. Không prompt: −1,5.
- **Sửa trực tiếp, không probe và không library:** −10,2 (Code), −4,9 (Web).

**Vì sao nên giảm trọng số:**
1. Preprint mới 11 ngày, chưa ai chạy lại. Wavect chỉ đọc code, không chạy ([Wavect](https://wavect.io/blog/google-harness2-setup-cost-safety/)).
2. n nhỏ: ở JobBench một task tương đương khoảng 2,9 điểm, và SD lên tới 4,5–6,9.
3. Toàn bộ do LLM chấm. Opus 4.8 vừa là editor vừa là judge của JobBench, nên có thể thiên vị chính nó (đây là suy luận của tôi).
4. Đây là adaptation ngay trên task được chấm: editor đã xem 3 rollout của chính task đó trước lần chạy được tính điểm. Chi phí ≥ 4× rollout cộng 3 phiên Opus, và paper không đưa tổng $ hay token trong phần đọc được.
5. Mạng mở: một rollout đã tự tải rubric trong lúc validate. Trong phần paper tôi đọc được, không thấy nói các run chính có chặn mạng hay không.

### 4. Cơ chế, so với INIT

| Cơ chế Harness² (file) | INIT hiện có | Kết luận |
|---|---|---|
| Mọi edit phải trỏ được tới một khoảnh khắc trong trajectory; thứ tự ưu tiên: khác biệt giữa các run > lỗi lặp lại > rủi ro tiềm ẩn ([prompts.py](https://raw.githubusercontent.com/google-research/harness2/main/harness2/core/prompts.py)) | `Repro:` bắt buộc bằng máy cho `fix`, `guard` và mọi thay đổi prefix (evolution.md §3.8) | INIT mạnh hơn vì kiểm bằng máy, nhưng hẹp hơn: thân skill không cần bằng chứng → ý tưởng #1 |
| Tách proposer INV/DOM ([recursion.py](https://raw.githubusercontent.com/google-research/harness2/main/harness2/core/recursion.py)) | Không có; agent tự quyết | **Thiếu** → #3 |
| Library lesson có NO_ENTRY; mỗi edit được gắn nhãn helped / hurt / no effect / confounded ([library.py](https://raw.githubusercontent.com/google-research/harness2/main/harness2/core/library.py)) | `refs/notes/predict` do kernel chấm (effective / partial / harmful …), lineage | INIT có bằng chứng mạnh hơn nhưng **thiếu lớp chưng cất có index** → #2 |
| Probe trên cùng task, so contrast không cần nhãn | G4 có nhãn: pool, holdout, canary, paired bootstrap | INIT mạnh hơn hẳn khi quyết accept; thiếu tín hiệu cho task không có check → #8 |
| Compose, merge nhiều candidate | Không có (L2 dùng một committer duy nhất) | Thiếu → #7 (L2) |
| Ledger `assert_identity` (R = 4k, P = 3k, kiểm trùng trước) | RunRecord, ngưỡng tối thiểu, chạy lại khi `infra` | Thiếu assert tường minh → #5 |
| Marker hoàn tất kèm fingerprint ([runmark.py](https://raw.githubusercontent.com/google-research/harness2/main/harness2/core/runmark.py)) | Kernel restart giữa gate thì reject và xếp hàng lại, không resume dở dang | Chưa cần. Bài học từ #65: nếu sau này có resume, fingerprint phải gồm cả input của task |
| Method không thấy score; judge chạy sau selection | Holdout chỉ kernel thấy; S do Root niêm phong; cách ly ở mức OS | **INIT tốt hơn** |
| Tripwire chuỗi verdict của grader ([context.py](https://raw.githubusercontent.com/google-research/harness2/main/harness2/core/context.py)) | Canary (điểm tuyệt đối nghĩa là gian lận), mutation suite M | INIT tốt hơn |
| Trần always-on, "thêm thì phải cắt" | Hash prefix ở G2, `Repro:` khi đổi prefix, `body_tokens` dùng để tie-break, lint chống nội dung hay thay đổi trong head | INIT tốt hơn |
| `obs_trunc` ≥ 10k, vì cắt ngắn hơn sẽ "bịa ra lỗi" | Output lớn được spill ra file, không mất dữ liệu | INIT đã có |
| Plugin và hook trong tool loop (C8) | Hook WIT được hoãn có chủ đích (ext/wit.md §3.5) | Giữ nguyên quyết định hoãn: paper không ablate riêng plugin |
| Không có trần chi phí (#4), đếm thiếu chi phí (#50) | Root R2: trần cứng theo ngày và tổng | INIT tốt hơn |
| Guardrail có thể bị editor xoá (#94) | Charter và Root bất biến | INIT tốt hơn |
| Demo $0 với proposer viết sẵn kịch bản ([demo.py](https://raw.githubusercontent.com/google-research/harness2/main/harness2/demo.py)) | Mock LLM; A2–A6 chạy ở $0 | Tương đương |

### 5. Có gì thách thức thiết kế lõi?

- **Tự sửa đổi (T2).** Harness² đạt +5 đến +11 điểm mà **không đụng tới runtime**, chủ yếu nhờ skills và scripts. Điều này không bác bỏ T2, vì Harness² vốn không được sửa core. Nhưng nó là tín hiệu ưu tiên: sau v0.1, nên đầu tư vào chất lượng vòng T0/T1 trước khi làm thêm máy móc cho T2. AHE trước đây cũng chỉ ra tools, middleware và memory quan trọng hơn prompt (reports/Agentic harness tự tiến hoá.md §2).
- **Root.** Được củng cố. Không có lớp bất biến thì guardrail có thể bị xoá (#94), chi phí không có trần (#4), và mạng mở dẫn tới việc tải được rubric.
- **Gate thống kê.** Được củng cố. Sửa không có gate thì tụt 10 điểm. 9 trường hợp regression đến từ chọn sai candidate và nhiễu, đúng loại lỗi mà paired bootstrap bắt được. **Thách thức thật:** G4 chỉ đo được task có check tất định. Ở các miền như luật hay văn phòng, nơi Harness² hoạt động, INIT sẽ kẹt ở `no_gain` hoặc `underpowered`. INIT nhắm vào coding nên vẫn chấp nhận được, nhưng chủ cần biết giới hạn này. Contrast không nhãn chỉ nên dùng để đề xuất (ý tưởng #8).
- **Rust + Wasm.** Không bị thách thức. Harness² là orchestrator Python gọi các CLI Node, và plugin TS chạy in-process không có sandbox. T1 Wasm cộng sandbox OS của INIT an toàn hơn.
- **Reset về H0 mỗi task.** Harness² không tự động kế thừa component nào giữa các task, chỉ kế thừa lesson. Cách này tránh tích tụ rác. INIT tích luỹ vào LKG nhưng có retirement, thinning và sealed audit, nên không cần đổi.

### 6. Ảnh hưởng tới v0.1

**Không chặn release.** v0.1 gồm L0, promotion T1/T2 thật và kiểm tra an toàn. Lỗ hổng duy nhất tìm thấy (thân skill không cần bằng chứng và không có G4) là vấn đề chất lượng tiến hoá, không phải an toàn, và đường T0 không nằm trong tiêu chí hoàn thành của v0.1 (RELEASE_PLAN.md). Ý tưởng #1 rẻ (2–3 agent-h). Nếu muốn, có thể đưa vào v0.1.x ngay sau khi tag.

## Nguồn

- Repo, README: https://github.com/google-research/harness2
- GitHub API (metadata, commits, tree): https://api.github.com/repos/google-research/harness2
- Paper: https://arxiv.org/abs/2610.14224 · HTML: https://arxiv.org/html/2610.14224
- Trang dự án: https://harness2.github.io
- Source: `harness2/core/{recursion,tree,context,library,ledger,selection,judge,spec,runmark,schedule,proposer,prompts}.py`, `harness2/substrate/{opencode,codex}.py`, `harness2/{demo,preflight}.py` tại https://raw.githubusercontent.com/google-research/harness2/main/
- Docs: https://raw.githubusercontent.com/google-research/harness2/main/docs/EXPERIMENTS.md
- Issues: https://github.com/google-research/harness2/issues (#4, #50, #65, #80, #94)
- Wavect (bài review thứ cấp, không có số đo riêng): https://wavect.io/blog/google-harness2-setup-cost-safety/
