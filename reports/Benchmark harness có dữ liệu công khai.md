# Tự chạy đối chứng, đừng mượn leaderboard

**Chưa benchmark nào công bố kết quả per-task của harness khác trên đúng `deepseek-flash` hiện nay.** Từ 17/10/2026 alias này trỏ vào DeepSeek-V4.1-Flash-1017. Kết quả độc lập duy nhất cho snapshot đó chỉ là một con số tổng: 31,82% trên Terminal-Bench 4.0 với mini-swe-agent, do Vals chạy. Nguồn có nhiều dữ liệu DeepSeek theo từng instance nhất là SWE-bench Verified. Ở đó 175 entry công khai danh sách task giải được, trong đó có mini-SWE-agent + DeepSeek V3.2 (hai run, kèm chi phí và số lượt gọi từng task), OpenHands + V3.2-Reasoner và V4-Pro (qua OpenHands Index), và SWE-Exp, Agentless, SWE-agent trên V3/R1. Terminal-Bench 2.0 có dữ liệu per-trial tốt hơn (Terminus 2 + V3.2, 5 trial mỗi task). SWE-bench-Live có Claude Code, OpenHands, SWE-agent trên V4-Pro và TianxiCode trên V4.1-Flash. Cả hai đều bắt buộc chạy trong container Linux. Vì model đã khác, dữ liệu công khai chỉ dùng để chọn task và làm bối cảnh cho từng task. **Đối chứng đáng tin rẻ nhất là tự chạy một arm tham chiếu là mini-swe-agent: cùng snapshot, cùng task, cùng môi trường, xen kẽ theo thời gian.**

Đề xuất cụ thể:
- **Task:** 12 task SWE-bench Verified ở vùng khó trung bình, thuần Python (django, sympy, pytest, sphinx), chạy native dưới Seatbelt.
- **Lượt chạy:** 3 lượt mỗi task mỗi arm, tổng 72 lượt.
- **Chi phí:** khoảng **$0,7–3,6 nếu chạy giờ off-peak, trần cứng $4**.
- **Chấm:** bằng grader native đã qua kiểm tra oracle, rồi đối chiếu chéo một lần qua sb-cli. Cả quy trình không cần Docker.

Thiết kế này cho ra một con số thật: pass rate kèm khoảng tin cậy, $/task và token/task. Nhưng nó **không đủ để kết luận INIT hơn hay kém về pass rate**. Với 12 task × 3 lượt, phép thử chỉ có khoảng 77% cơ hội phát hiện chênh lệch thật cỡ 30 điểm. Trong khi đó, chính DeepSeek đo được chênh lệch giữa 8 harness trên V4.1-Flash chỉ 6,5–8,7 điểm. Docker chỉ cần ở giai đoạn 2: Terminal-Bench qua Harbor, SWE-bench-Live, hoặc chấm SWE-bench chính thức tại máy. Máy hiện chỉ còn 10 GB trống, dưới ngưỡng 20 GB anh đặt.

## Không nguồn nào có số per-task cho snapshot 1017, và SWE-bench Verified có nhiều dòng DeepSeek nhất

Cái tên `deepseek-flash` không trỏ cố định vào một model. DeepSeek chuyển alias này sang V4.1-Flash ngày 10/9/2026, rồi sang V4.1-Flash-1017 ngày 17/10. Các tên cũ như `deepseek-v4-flash` cũng được âm thầm chuyển sang V4.1 Flash ([DeepSeek changelog](https://api-docs.deepseek.com/updates/); [DeepSeek pricing](https://api-docs.deepseek.com/quick_start/pricing)). Hệ quả đo được rất lớn. Cùng harness mini-swe-agent, cùng Terminal-Bench 4.0, bản V4.1 Flash tháng 9 đạt 19,70%, còn bản 1017 đạt 31,82%. Tức là **chênh 12 điểm dù vẫn gọi cùng một alias**, lớn hơn mọi chênh lệch harness có kiểm soát ở phần dưới ([Vals TB4](https://www.vals.ai/benchmarks/terminal-bench-4)). Con số 31,82% (avg@3, sandbox Daytona, $0,54 mỗi lượt, trung bình 41 phút) là kết quả độc lập duy nhất cho đúng snapshot INIT đang gọi, và nó chỉ là số tổng. Mọi kết quả công khai ghi "V4 Flash", hoặc có ngày trước 17/10, đều thuộc một model khác.

Gần snapshot hiện tại nhất là TianxiCode (CLI của Lenovo) + DeepSeek-v4.1-Flash trên SWE-bench-Live Lite: **213/300 = 71,0%, đã verified, ngày 8/10/2026**. Run này dùng một rollout, không best-of-N, chạy trong container chính thức, và danh sách instance nằm trong `results.json` của submission ([reports-0605.jsonl](https://github.com/SWE-bench-Live/swe-bench-live.github.io/blob/main/reports-0605.jsonl); [TianxiCode README](https://github.com/SWE-bench-Live/submission/tree/main/submissions/lite/tianxicode/deepseek-flash)). Nhưng đó vẫn là bản V4.1-Flash tháng 9. Các run Java, Go (16/10) và Rust (20/10) của cùng nhóm nằm sát ngày đổi alias, nên không xác định được chúng dùng snapshot nào. Phép so sánh harness có kiểm soát duy nhất trên V4.1-Flash là của chính DeepSeek: 8 harness, effort max, cùng container. Nhưng DeepSeek chỉ công bố số tổng ([V4.1-Flash card](https://huggingface.co/deepseek-ai/DeepSeek-V4.1-Flash)). Bảng dưới xếp các nguồn theo mức hữu dụng cho INIT.

| Nguồn | Dữ liệu per-task công khai | Dòng DeepSeek (harness, điểm) | Harness khác có per-task | Chạy trên Mac không cần Docker? | Vai trò cho INIT |
|---|---|---|---|---|---|
| SWE-bench Verified ([experiments](https://github.com/SWE-bench/experiments/blob/main/README.md)) | Danh sách resolved cho 175/182 entry; cost + số lượt gọi từng task cho 41 run mini | mini + V3.2 Reasoner 60,0%; mini + V3.2 high 70,0%; SWE-Exp + V3 42,0%; Agentless/OpenHands/SWE-agent + V3, R1 (2025) ([Multi-SWE-bench python/verified](https://github.com/multi-swe-bench/experiments/tree/main/evaluation/python/verified)) | 77 agent, gồm OpenHands, SWE-agent, Moatless, Agentless; không có Claude Code, Codex, OpenCode | Agent: có, nếu tự dựng môi trường native. Chấm: sb-cli trên cloud, miễn phí | **Nguồn chính** |
| OpenHands Index ([results](https://github.com/OpenHands/openhands-index-results)) | resolved + cost từng instance trên Verified, SWT, SWE-MM, Commit0, GAIA | OpenHands + V4-Pro 73,2% ($0,040/instance); OpenHands + V3.2-Reasoner 71,6% ($0,16) | Claude Code, Codex qua ACP, nhưng chỉ trên model của chính hãng | Như trên (cùng ID Verified) | Bổ sung cho dòng trên |
| SWE-bench-Live ([submission](https://github.com/SWE-bench-Live/submission/blob/main/README.md)) | Per-instance trong `results.json` từng submission | TianxiCode + V4.1-Flash Lite 71,0%; Claude Code/OpenHands/SWE-agent + V4-Pro, ví dụ TS/JS 63/75/75 trên 204 | Claude Code fork, OpenHands, SWE-agent | Không: image riêng, chấm cần Docker | Giai đoạn 2 |
| Terminal-Bench 2.0 ([HF](https://huggingface.co/datasets/harborframework/terminal-bench-2-leaderboard/tree/main/submissions/terminal-bench/2.0/Terminus2__DeepSeek-V3.2)) | Reward + cost từng trial, 5 trial mỗi task | Terminus 2 + V3.2: 39,6%, $0,008–0,053 mỗi trial | 76 thư mục submission trên HF | Không: Harbor không có backend native | Giai đoạn 2 |
| Terminal-Bench 2.1 / 3.0 / 4.0 ([Harbor hub](https://hub.harborframework.com)) | Per-trial | Không có dòng DeepSeek; chỉ có số tổng của Vals cho bản 1017 | Claude Code, Codex, Terminus 2, mini, Gemini CLI, Cursor CLI | Không | Chỉ làm bối cảnh |
| Multi-SWE-bench ([experiments](https://github.com/multi-swe-bench/experiments/blob/main/README.md)) | Per-instance | MSWE-agent/MagentLess/MopenHands + V3, R1 (2025) | 3 bản port của ByteDance | Không | Dữ liệu cũ |
| Aider Polyglot ([YAML](https://github.com/Aider-AI/aider/blob/main/aider/website/_data/polyglot_leaderboard.yml)) | Chỉ có số tổng | aider + V3.2-Exp 70,2–74,2% | Không có | Có (34 bài Python + 30 bài Rust) | Smoke test, không so được per-task |
| HAL, METR ([HAL](https://hal.cs.princeton.edu/); [METR](https://github.com/METR/eval-analysis-public)) | HAL: trace mã hoá; METR: runs.jsonl | HAL: V3, R1, V3.1 trên nhiều scaffold; METR: R1, V3 | Nhiều | Không | Không dùng |

SWE-bench Verified là nguồn duy nhất đáp ứng cả năm điều kiện sau:
- có kết quả từng instance;
- có nhiều harness khác nhau chạy model DeepSeek;
- có chi phí từng task;
- tải về không cần tài khoản;
- phần agent chạy được ngoài Docker.

Claude Code, Codex và OpenCode không có dòng nào trên mọi bảng SWE-bench ([leaderboards.json](https://github.com/SWE-bench/swe-bench.github.io/blob/master/data/leaderboards.json)). Chúng chỉ xuất hiện trên SWE-bench-Live, Terminal-Bench và các dòng ACP của OpenHands Index, và chỉ SWE-bench-Live ghép chúng với model DeepSeek.

Các nguồn còn lại đều vướng điều kiện nào đó:
- Aider Polyglot chạy native được, nhưng chỉ công bố số tổng và không có kết quả mới từ cuối 2025.
- HAL đã tạm dừng thêm model và mã hoá trace ([HAL traces](https://huggingface.co/datasets/agent-evals/hal_traces)).
- Task của METR phần lớn không công khai hoặc đã deprecated ([METR/hcast-public](https://github.com/METR/hcast-public)).

Một ràng buộc thực tế: từ 18/11/2025, bảng Verified chính thức chỉ nhận submission từ nhóm học thuật có công bố. Vì vậy INIT không lên được bảng đó, và dữ liệu Verified chỉ dùng được để đối chiếu riêng ([experiments README](https://github.com/SWE-bench/experiments/blob/main/README.md)).

## SWE-bench Verified chạy native được, Terminal-Bench thì bắt buộc container

Mỗi entry trong `SWE-bench/experiments` có file `results/results.json` liệt kê các instance đã giải. 41 run mini-SWE-agent còn có thêm `per_instance_details.json`, ghi `api_calls`, `cost` và `resolved` cho từng task. Patch, log và trajectory tải được từ S3 công khai hoặc từ repo của người nộp, không cần credentials ([experiments README](https://github.com/SWE-bench/experiments/blob/main/README.md)). Hai run DeepSeek V3.2 trong mini là mốc rẻ nhất:
- Bản Reasoner (mini v1.17.1) đạt **60,0% với $0,028 mỗi task, trung vị 41 lượt gọi** ([per_instance_details.json](https://github.com/SWE-bench/experiments/blob/main/evaluation/verified/20251201_mini-v1.17.1_deepseek-v3.2-reasoner/per_instance_details.json)).
- Bản high effort (mini v2.0.0) đạt **70,0% với $0,448 mỗi task**, tức đắt gấp 16 lần ([metadata.yaml](https://github.com/SWE-bench/experiments/blob/main/evaluation/verified/20260217_mini-v2.0.0_deepseek-3-2-high/metadata.yaml)).

OpenHands Index bổ sung kết quả theo cùng ID instance cho OpenHands + V4-Pro (366/500) và OpenHands + V3.2-Reasoner ([DeepSeek-V4-Pro](https://github.com/OpenHands/openhands-index-results/tree/main/results/DeepSeek-V4-Pro)). Hai run V3.2 dù cùng họ harness vẫn **bất đồng trên 100/500 = 20% instance**. Đó là sàn nhiễu mà mọi so sánh per-task đều phải tính tới ([run high](https://github.com/SWE-bench/experiments/tree/main/evaluation/verified/20260217_mini-v2.0.0_deepseek-3-2-high)).

Độ khó phân bố rất lệch. Trong 175 entry, 26 instance không ai giải và 85 instance chỉ dưới 10% entry giải được. Có 74 instance được 35–65% của 75 entry từ tháng 7/2025 trở đi giải được, phần lớn là django (29) và sympy (16) ([SWE-bench/experiments](https://github.com/SWE-bench/experiments/tree/main/evaluation/verified)). Vùng giữa này là nơi mỗi task cho nhiều thông tin nhất. Nên tránh vùng "không ai giải": OpenAI audit 138 task khó nhất và thấy 59,4% trong đó có lỗi test hoặc lỗi đề nghiêm trọng. OpenAI ngừng báo cáo Verified từ 23/2/2026 vì model frontier tái hiện được bản sửa gốc ([OpenAI](https://openai.com/index/why-we-no-longer-evaluate-swe-bench-verified/); [AI Weekly](https://aiweekly.co/alerts/openai-drops-swe-bench-verified-backs-pro-after-flaw-audit)). Với INIT, việc model đã thấy dữ liệu làm phồng điểm tuyệt đối của deepseek-flash. Nhưng nó tác động như nhau lên mọi arm dùng cùng model, nên so sánh tương đối trên cùng task vẫn dùng được, với điều kiện chặn đường tra lời giải. Vals từng bắt được model đọc git history còn sót trong task Terminal-Bench 4 ([Vals AI](https://vals.ai/blogs/mimo-reward-hacking)).

Có bốn cách chấm, và chỉ một cách cần Docker trên máy:
- **sb-cli:** chấm miễn phí trên cloud của SWE-bench, nhận `--instance_ids` để chấm tập con, và thường trả kết quả trong khoảng 20 phút. Hạn chế là quota tính theo tài khoản: ví dụ trong tài liệu chỉ cho **1 lượt nộp với `swe-bench_verified test`**, và repo không có commit nào từ tháng 8/2025 ([get-quotas](https://github.com/SWE-bench/sb-cli/blob/main/docs/user-guide/get-quotas.md); [mini-swe-agent docs](https://github.com/SWE-agent/mini-swe-agent/blob/main/docs/usage/swebench.md); [sb-cli commits](https://github.com/SWE-bench/sb-cli/commits/main)).
- **Harness v5 chạy local:** cần Docker. Hỗ trợ arm64 được ghi là "experimental" và phải build image tại máy bằng `--task-repo` ([SWE-bench README](https://github.com/SWE-bench/SWE-bench/blob/main/README.md)).
- **Modal:** chạy được nhưng tốn tiền.
- **Grader native (báo cáo này đề xuất):** áp test patch và patch của agent, rồi chạy đúng các test FAIL_TO_PASS và PASS_TO_PASS của instance trong một venv trên Mac. Trước đó phải kiểm rằng trên từng task, gold patch qua và patch rỗng trượt. Lượt nộp sb-cli duy nhất dùng để đối chiếu chéo grader này.

Cái giá của grader native là môi trường không trùng với image x86 mà các entry công khai đã dùng. Vì vậy khi đặt cạnh số công khai, kết quả của INIT phải gắn nhãn "native-env".

Terminal-Bench có dữ liệu per-task tốt hơn về mặt thống kê. Run Terminus 2 + DeepSeek-V3.2 trên TB 2.0 có 5 trial cho mỗi task trong 89 task. Trong 442 trial được chấm, 176 trial qua và 90 trial lỗi AgentTimeoutError. Mỗi trial tốn $0,008–0,053, và cả run mất khoảng 24 giờ với 4 trial song song ([HF job](https://huggingface.co/datasets/harborframework/terminal-bench-2-leaderboard/tree/main/submissions/terminal-bench/2.0/Terminus2__DeepSeek-V3.2)). 22 dòng TB 2.1 có dữ liệu per-trial khớp điểm báo cáo trong phạm vi ±0,9 điểm ở 21/22 dòng ([Harbor hub](https://hub.harborframework.com)).

Nhưng Harbor có 34 backend và không backend nào chạy native. Apple `container` chỉ hỗ trợ macOS 26, còn máy anh chạy macOS 14.6 ([harbor environments](https://github.com/harbor-framework/harbor/tree/main/src/harbor/environments); [apple/container](https://github.com/apple/container)). Chạy Terminal-Bench vì vậy kéo theo ba hệ quả:
- phải dùng Docker Desktop, với image x86 chạy giả lập trên Apple Silicon (chưa kiểm từng task);
- lệnh của agent chạy trong container, nên **Seatbelt của INIT hoàn toàn không được kiểm tra**;
- INIT cần thêm một adapter để gửi lệnh vào container.

Kiểm tra máy ngày 21/10 cho thấy Docker Desktop 27.4 đang chạy với VM 8 GB RAM và 5 container của dự án khác, còn ổ đĩa chỉ trống 10 GB. TB 3.0 và 4.0 càng không hợp: mỗi trial dài khoảng một giờ, và theo chính số của DeepSeek, V4.x Flash chỉ đạt 7–42% trên hai bảng này ([V4.1-Flash-1017 card](https://huggingface.co/deepseek-ai/DeepSeek-V4.1-Flash-1017)).

## Mười hai task chỉ bắt được chênh lệch cỡ 30 điểm

Trước khi thiết kế cần ước lượng hiệu ứng harness lớn cỡ nào. Trên V4.1-Flash, 8 harness của DeepSeek cách nhau 6,5 điểm trên TB 2.1: mini-SWE 90,3, Claude Code 88,0, Pi 86,1, OpenCode 85,0, Codex 84,1. Trên DeepSWE v1.1 khoảng cách là 8,7 điểm: mini-SWE 74,2, Claude Code 69,8, OpenCode 65,5 ([V4.1-Flash card](https://huggingface.co/deepseek-ai/DeepSeek-V4.1-Flash)). Các so sánh có kiểm soát trên TB 2.1 với model frontier cũng chỉ chênh 0,2–8,1 điểm ([TB 2.1](https://www.tbench.ai/?version=2.1)). Những khoảng chênh 13–21 điểm trên TB 2.0 đến từ các submission khác nhau về prompt, phiên bản và ngân sách, chứ không phải từ thí nghiệm có kiểm soát ([Guo et al.](https://arxiv.org/abs/2606.20683)). Giả định hợp lý nhất vì thế là trên deepseek-flash, INIT và một harness tối giản tốt sẽ cách nhau dưới 10 điểm. Với chính model này, harness tối giản thắng harness nặng, nên mini-swe-agent là mốc khó vượt hơn Claude Code.

Với mẫu nhỏ, phép kiểm định phù hợp là McNemar mid-p hoặc kiểm định hoán vị đổi dấu trên chênh lệch theo từng task ([Fagerland et al. 2013](https://www.ncbi.nlm.nih.gov/pmc/articles/PMC3716987/); [Miller 2024](https://arxiv.org/html/2411.00640v1)). Ở n = 5, kể cả khi INIT thắng cả 5 task bất đồng, p hai phía chính xác vẫn là 0,0625. Tức là **5 task không bao giờ đạt ý nghĩa thống kê**. Bảng dưới là độ mạnh kiểm định từ mô phỏng chạy riêng cho báo cáo này, với giả định: tỉ lệ giải nền theo phân phối Beta(0,5; 0,5); INIT được cộng một độ dịch logit cố định; kiểm định hoán vị đổi dấu với α = 0,05; 1.000 lần mô phỏng mỗi ô.

| Chênh lệch thật về pass rate | 12 task × 3 lượt (đề xuất) | 20 task × 5 lượt |
|---|---|---|
| +5 điểm | 2% | 10% |
| +10 điểm | 7% | 38% |
| +19 điểm | 29% | 89% |
| +25 điểm | 53% | 99% |
| +32 điểm | 77% | gần 100% |

Nếu khác biệt thật chỉ ở mức 5–10 điểm như dữ liệu của DeepSeek gợi ý, thiết kế 12 × 3 gần như chắc chắn không phân biệt được INIT với mini-swe-agent. Ngay cả 20 × 5, tốn gần gấp 3 lần, cũng chỉ có 38% cơ hội phát hiện chênh lệch +10 điểm. Nhiễu còn cộng thêm. Theo mô hình, hai lần chạy cùng một hệ thống sẽ cho kết quả khác nhau trên 19–33% task, khớp với mức 20% đo được giữa hai run DeepSeek V3.2. DeepSWE đã thử đúng cỡ mẫu này: 10 task SWE-Bench Pro, harness native so với mini-swe-agent. Chính các tác giả kết luận là không phân biệt được ([DeepSWE](https://arxiv.org/html/2607.07946v1)).

Thứ đo được chắc chắn ở cỡ mẫu này là hiệu quả. Số token và tiền trên mỗi task là biến liên tục, ít nhiễu hơn nhiều so với kết quả đậu/trượt. Trên DeepSeek V4 Pro, Composio thấy 6 harness chỉ chênh nhau 3 trên 30 task về số task qua. Nhưng Claude Code dùng **649.900 token mỗi task, so với 88.562 của DeepSeek Harness, gấp 7,3 lần** ([Composio](https://composio.dev/content/deepseek-harness-vs-claude-code)). "Cùng tỉ lệ giải nhưng tốn X% token" là claim có nhiều khả năng đứng vững nhất cho INIT. Nó cũng là claim ở cấp hệ thống. Theo một phân tích phương sai trên khoảng 30 nghìn rollout của HAL, thứ hạng của một hệ thống cố định (model + scaffold) tin cậy hơn nhiều so với thứ hạng của model hay của harness nói chung ([Hardy et al.](https://arxiv.org/abs/2610.00651), đọc qua bản tóm tắt).

| Claim | Được nói không | Điều kiện hoặc lý do |
|---|---|---|
| "INIT + deepseek-flash (1017, effort X, ngày Y) giải k/12 task đã đăng ký trước, CI 95% [a, b]" | Được | Task đăng ký trước khi chạy và snapshot được ghi lại. Ví dụ 8/12 cho Wilson [39, 86]% |
| "Trên task T, INIT giải 3/3, còn mini + V3.2 (đã công bố) trượt" | Được, nhưng chỉ là giai thoại | Đính kèm trajectory đã audit, không cộng dồn thành claim "INIT giỏi hơn" |
| "INIT tốn X token/task, bằng Y% của mini-swe-agent, cùng snapshot, cùng task, cùng khung giờ" | Được | Biến liên tục, ít nhiễu, là claim chính nên nhắm tới |
| "INIT chênh mini-swe-agent Δ điểm (CI bootstrap, p hoán vị)" | Được, ở cấp hệ thống | Chỉ kết luận khi \|Δ\| cỡ 30 điểm trở lên, dưới mức đó báo "không phân biệt được" |
| "INIT hơn Claude Code hay OpenHands" dựa trên số công bố | Không | Khác model, snapshot, effort, giới hạn và môi trường; số leaderboard là best-of-submission |
| Ngoại suy điểm toàn bộ Verified từ 12 task | Không | Task được chọn có chủ đích ở vùng giữa |
| So pass@3 của INIT với pass@1 của harness khác | Không | pass@k làm phồng điểm; nên báo pass@1 trung bình và pass^3 ([τ-bench](https://arxiv.org/abs/2406.12045)) |
| Coi kết quả "V4 Flash" cũ là cùng model | Không | Alias đã đổi hai lần trong năm tuần |

Mỗi claim "Được" trong bảng chỉ đứng vững khi các yếu tố gây nhiễu được cố định:
- **Effort giống nhau giữa các arm.** DeepSeek có các mức low/high/max từ 13/8/2026. Endpoint Anthropic chỉ nhận `output_config.effort` và bỏ qua `thinking.budget_tokens` ([DeepSeek Anthropic API](https://api-docs.deepseek.com/guides/anthropic_api)).
- **Giới hạn bước, thời gian và chi phí giống nhau** giữa các arm.
- **Snapshot ghi từ phản hồi API**, và hai arm chạy xen kẽ trong cùng khung giờ, vì alias có thể đổi giữa chừng.
- **Audit tay mọi task bất đồng** (đọc trajectory và patch) trước khi tính, vì grader SWE-bench từng cho 5,2% task qua mà không cần patch đúng. Cần thêm một baseline patch rỗng ([ABC](https://arxiv.org/html/2507.02825v5)).
- **Coi số leaderboard là cận trên**, không phải một lần rút ngẫu nhiên ([Leaderboard Illusion](https://arxiv.org/abs/2504.20879)).

## Kế hoạch đề xuất: 72 lượt native, trần $4, Docker để sang giai đoạn 2

Bảng dưới so năm thiết kế, xếp từ rẻ đến đắt. Thiết kế B là thiết kế rẻ nhất có nhóm đối chứng cùng snapshot, nên báo cáo này đề xuất B.

| Thiết kế | Số lượt | API (off-peak) | Docker | Kết luận được phép |
|---|---|---|---|---|
| A. INIT chạy một mình, đối chiếu dữ liệu công khai | 36 | $0,4–1,8 | Không | Mô tả, giai thoại per-task |
| **B. A + mini-swe-agent cùng snapshot (đề xuất)** | 72 | $0,7–3,6 | Không | Thêm so sánh có kiểm soát ở cấp hệ thống và so token/$ |
| C. B + Claude Code, mỗi task 1 lượt | 84 | Thêm tối đa $1 | Không | Thêm mốc "vendor CLI" cùng snapshot, chỉ mô tả |
| D. Terminal-Bench 2.0 qua Harbor: INIT + Terminus 2 | 72 trial | $0,7–3,6 (chưa kiểm) | Có | Thêm so per-trial với Terminus 2 + V3.2 đã công bố |
| E. 20 task × 5 lượt × 2 arm | 200 | $2–10 | Không | Phát hiện được chênh lệch từ khoảng 20 điểm |

Arm tham chiếu nên là mini-swe-agent chứ không phải Claude Code, vì năm lý do:
- Các run DeepSeek per-instance đã công bố trên Verified đều chạy bằng mini, nên mini là cầu nối sang dữ liệu công khai.
- mini đứng đầu hoặc nhì trong sweep 8 harness của DeepSeek.
- Vals và Scale dùng mini làm harness chuẩn cho mọi model ([Scale SWE-Bench Pro](https://labs.scale.com/leaderboard/swe_bench_pro_public)).
- mini có môi trường `local` chạy lệnh trực tiếp trên máy ([mini-swe-agent docs](https://github.com/SWE-agent/mini-swe-agent/blob/main/docs/usage/swebench.md)).
- mini rẻ.

Lý lẽ mạnh nhất cho Claude Code cũng có thật:
- Nó đã được cài sẵn (2.1.261).
- DeepSeek có hướng dẫn chính thức cho nó ([Claude Code guide](https://api-docs.deepseek.com/quick_start/agent_integrations/claude_code/)).
- DeepSeek dùng chính nó cho con số 42,3 trên TB 4.0.
- "INIT so với Claude Code" là câu hỏi người đọc quan tâm nhất.

Nhưng Claude Code dùng token gấp nhiều lần. Endpoint của DeepSeek bỏ qua `cache_control`, và mọi tên `claude-opus*` không được ghim sẽ bị chuyển sang V4-Pro, đắt hơn khoảng 3,3 lần. Vì vậy Claude Code chỉ nên là arm tuỳ chọn, mỗi task 1 lượt, có trần riêng. OpenCode nên bỏ qua: nó yếu nhất trong sweep của DeepSeek và không có dữ liệu per-instance công khai nào.

Mười task vùng giữa dưới đây lấy từ một lần rút ngẫu nhiên 15 task trong 74 task vùng 35–65% ([SWE-bench/experiments](https://github.com/SWE-bench/experiments/tree/main/evaluation/verified)). Sau đó áp một quy tắc lọc không phụ thuộc agent: chỉ giữ repo chạy native được, không cần mạng và không cần build C extension. Quy tắc này loại matplotlib, seaborn, hai task scikit-learn và requests. Thêm hai task "anchor" mà ≥ 90% entry giải được, chọn bằng script trong cùng các repo, để kiểm tra môi trường. Task nào trượt kiểm tra oracle sẽ được thay bằng task kế tiếp theo seed, trước khi bất kỳ arm nào chạy. Hai cột kết quả cho thấy chính hai run V3.2 bất đồng trên 4/10 task.

| Task | mini + V3.2 Reasoner | mini + V3.2 high |
|---|---|---|
| django__django-11848 | 1 | 1 |
| django__django-12965 | 0 | 1 |
| django__django-14404 | 1 | 1 |
| django__django-16938 | 0 | 1 |
| pytest-dev__pytest-6197 | 0 | 0 |
| sphinx-doc__sphinx-7454 | 1 | 1 |
| sympy__sympy-13877 | 0 | 0 |
| sympy__sympy-13878 | 0 | 1 |
| sympy__sympy-15875 | 0 | 0 |
| sympy__sympy-19040 | 0 | 1 |
| **Tổng** | **3/10** | **7/10** |

Mọi arm phải chạy trong điều kiện giống hệt nhau:
- **Checkout:** mỗi task một checkout chỉ chứa đúng commit gốc, đã xoá remote để không còn git history tương lai. Mỗi phiên bản repo một venv tạo bằng uv, đặt ngoài repo. Reset và clean checkout giữa các lượt.
- **Sandbox:** cùng một Seatbelt profile cho mọi arm (mini được bọc bằng `sandbox-exec`). Chỉ cho mạng ra `api.deepseek.com`.
- **Giới hạn:** 250 bước như cấu hình mặc định của mini, timeout 60 giây mỗi lệnh ([swebench.yaml](https://github.com/SWE-agent/mini-swe-agent/blob/main/src/minisweagent/config/benchmarks/swebench.yaml)), 30 phút mỗi lượt, trần $0,30 mỗi lượt.
- **Trần tổng $4, thực thi bên ngoài agent:** Root của INIT cho arm INIT, `MSWEA_GLOBAL_COST_LIMIT` cho mini.
- **Watchdog RAM theo tiến trình:** trong sprint trước, một tiến trình python trong sandbox phình lên 44 GB và đẩy ổ trống xuống 0,24 GB, vì Seatbelt không giới hạn bộ nhớ ([INIT proof sprint verdict](INIT%20proof%20sprint%20verdict.md)).
- **Model:** ghim `deepseek-flash` với cùng mức effort. Ghi snapshot từ phản hồi API. Chạy xen kẽ thứ tự arm ngẫu nhiên theo từng cặp task-lượt, trong giờ off-peak.

| Bước | Việc | API | Docker | Thời gian |
|---|---|---|---|---|
| 0 | Anh duyệt trần $4 cho DeepSeek, tự tạo API key sb-cli bằng email, và giải phóng ổ lên ≥ 20 GB | $0 | Không | ~15 phút của anh |
| 1 | Tải dữ liệu công khai (sparse clone experiments khoảng 11 MB, OpenHands Index), dựng ma trận per-instance, chọn anchor bằng script, commit file đăng ký trước (task, K, arm, metric, ngưỡng dừng) | $0 | Không | ~1 giờ |
| 2 | Dựng checkout và venv native cho 12 task, viết grader native, chạy kiểm tra oracle (gold qua, patch rỗng trượt) | $0 | Không | 1–2 giờ |
| 3 | Cài mini-swe-agent, cấu hình `local` + Seatbelt + watchdog, ghim model và effort | $0 | Không | < 1 giờ |
| 4 | Probe: 2 task × 1 lượt × 2 arm để đo $/lượt và phút/lượt. Nếu dự phóng vượt $3,5 thì hạ K xuống 2 (đã đăng ký trước) | $0,05–0,20 | Không | ~30 phút |
| 5 | Chạy chính 12 × 3 × 2 = 72 lượt, 3 lượt song song, giờ off-peak; dừng nếu alias đổi | $0,7–3,4 | Không | 2–6 giờ |
| 6 | Chấm native; nộp 12 patch lượt 1 của INIT lên sb-cli để đối chiếu grader | $0 | Không | ~1 giờ |
| 7 | Phân tích: bảng per-task đặt cạnh dữ liệu công khai, pass@1 với CI bootstrap theo task, pass^3, token/task, $/task giải được, kiểm định hoán vị, audit task bất đồng | $0 | Không | ~1 giờ |
| 8 (tuỳ chọn) | Arm Claude Code mỗi task 1 lượt, chỉ chạy nếu bước 5 tiêu ≤ $2,5 | ≤ $1 | Không | 1–2 giờ |

Ước tính chi phí dựa trên ba mốc:
- Trên bộ task nội bộ, INIT tốn trung bình **$0,0035 mỗi lượt** ([m7-results](../measurements/m7-results.md)). Đó là task nhỏ, nên chỉ là cận dưới.
- mini + V3.2 Reasoner tốn $0,028 mỗi task Verified, p90 $0,050.
- OpenHands + V4-Pro tốn $0,040 mỗi instance. Giá V4-Pro cao gấp 3,3–7,3 lần deepseek-flash tuỳ loại token ([DeepSeek pricing](https://api-docs.deepseek.com/quick_start/pricing)).

Từ đó, một lượt SWE-bench của harness gọn có lẽ tốn $0,02–0,10 theo giá peak, và một nửa mức đó ở giờ off-peak. Off-peak theo giờ Việt Nam là mọi giờ trong ngày thường trừ 08–11h và 13–17h, cộng cả cuối tuần. Rủi ro đuôi nằm ở effort cao: run V3.2 high tốn trung bình $0,448 mỗi task và tối đa $2,52. Vì vậy cần cả trần mỗi lượt lẫn bước probe. Thời lượng 5–15 phút mỗi lượt là ước tính chưa đo, và bước 4 sẽ đo nó. Nếu khởi động sau 17h, kết quả có vào sáng hôm sau, tổng cộng khoảng một ngày lịch.

Các bước sau bắt buộc Docker và chỉ nên làm ở giai đoạn 2:
- **Toàn bộ Terminal-Bench** (Harbor `-e docker`).
- **Mọi so sánh với SWE-bench-Live** (TianxiCode và Claude Code/OpenHands + DeepSeek).
- **Chấm SWE-bench chính thức tại máy**, chỉ khi sb-cli không dùng được và grader native lệch với nó.
- **Multi-SWE-bench.** Không khuyến nghị.

Nếu làm Terminal-Bench, nên chọn 10 task mà DeepSeek-V3.2 không giải 0/5 cũng không giải 5/5, và ít phải biên dịch hay tải nặng: adaptive-rejection-sampler (1/5), cancel-async-tasks (1/5), configure-git-webserver (2/5), custom-memory-heap-crash (4/5), feal-linear-cryptanalysis (2/5), mailman (4/5), path-tracing (1/5), pypi-server (4/5), query-optimize (2/5), winning-avg-corewars (2/5). Thêm hai anchor nginx-request-logging và multi-source-data-merger. Nên bỏ compile-compcert, qemu-startup và hf-model-inference vì rủi ro giả lập và dung lượng ổ. Quy trình cho giai đoạn này:
1. Chạy agent `oracle` của Harbor trên tập task trước.
2. Dùng Terminus 2 + deepseek-flash làm arm đối chứng. Đây cũng là harness của dòng DeepSeek đã công bố.
3. Cho INIT chạy qua adapter với `-k 3` ([harbor cli](https://github.com/harbor-framework/harbor/blob/main/src/harbor/cli/jobs.py)).

Chi phí $0,7–3,6 cho giai đoạn 2 dựa trên token của V3.2 ở chế độ non-thinking. deepseek-flash mặc định bật thinking nên có thể tốn hơn. Nếu giai đoạn 1 tiêu hết trần $4, số dư chỉ còn khoảng $2,9, nên giai đoạn 2 cần nạp thêm tiền hoặc giảm K.

## Kết luận

Giá trị thật của dữ liệu công khai cho INIT nằm ở việc chọn task và hiệu chỉnh độ khó. Nó không đóng được vai nhóm đối chứng. Ma trận 175 entry cho biết task nào thực sự phân loại được harness, và hai run DeepSeek V3.2 cho biết nhiễu nền lớn cỡ nào. Nhưng chỉ một arm tự chạy trong cùng khung giờ mới loại được yếu tố lớn nhất: snapshot đổi 12 điểm sau cùng một alias, lớn hơn mọi hiệu ứng harness đo được trên model này. Hệ quả cho thiết kế: ghi lại snapshot và chạy xen kẽ các arm quan trọng hơn việc thêm task. Với ngân sách còn lại, số liệu INIT có thể bảo vệ được là hiệu quả ở mức pass rate ngang nhau, không phải chiến thắng về pass rate.

Có một bước rẻ để biến con số này thành dữ liệu dùng lại được: xuất kết quả của INIT đúng định dạng `results.json` và `per_instance_details.json` của SWE-bench. Khi đó INIT ghép thẳng được vào ma trận công khai. Nếu sau này chuyển sang chấm trong Docker, cùng các file đó trở thành bằng chứng so sánh được với mọi entry khác. Nếu công bố, theo những gì nghiên cứu này tìm được, đó sẽ là bộ kết quả per-task công khai đầu tiên trên đúng snapshot 1017, và là bộ đầu tiên của một harness chạy native dưới Seatbelt.
