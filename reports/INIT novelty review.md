# INIT có đóng góp mới không? Rà soát prior art (tháng 10/2026)

*2026-10-20. Nghiên cứu web read-only, khoảng 20 truy vấn, đọc paper ở mức abstract hoặc HTML qua WebFetch. Có dùng lại ghi chú cũ đã kiểm chứng (DGM, HGM, Gödel Agent, SICA, Ouroboros, STOP, PROCTOR, Harness², Gated-QD). Ghi chú đầy đủ kèm URL: [research_notes/INIT novelty review/notes.md](../research_notes/INIT%20novelty%20review/notes.md). Số liệu của các paper là do tác giả tự báo cáo. "Không tìm thấy" nghĩa là các truy vấn của tôi không ra, không chứng minh là không tồn tại.*

## Kết luận

- **Không claim nào NOVEL về mặt ý tưởng. Cả ba đều INCREMENTAL**, tức là tổ hợp mới và kỹ thuật tốt từ các mảnh đã có prior art. Năm 2026 có nhiều paper chạm rất sát INIT: MOSS, Ouroboros, Unfireable Safety Kernel, SEAL, Red Queen Gödel Machine, "Who Verifies the Verifier?".
- **Xếp theo mức gần với mới:**
  1. **Claim 3 (gate-of-gates)** gần mới nhất. Kiểm grader bằng mutant và control là ý tưởng cũ (oracle assessment, ISSTA 2016). Từ tháng 9–10/2026 đã có người áp nó cho evaluator của agent. Nhưng tôi **không tìm thấy** hệ nào cho phép agent tự viết lại *gate thăng cấp của chính nó*, với điều kiện gate mới phải loại bỏ một bộ **agent mutant mô phỏng các kiểu reward hacking đã ghi nhận**, do một Root bất biến giữ.
  2. **Claim 2 (Root + autonomy ladder)**: mọi thành phần của Root đều đã có. Riêng ý "track record mở khoá **phạm vi tự sửa** mà không cần người duyệt" thì chưa thấy ai làm cho agent tự sửa mình. Cơ chế này lại chính là graduated autonomy của AWS (8/2026), áp sang self-modification.
  3. **Claim 1 (hot handoff)**: MOSS (5/2026) và Ouroboros (8/2026) đã làm gần hết vòng này. Mảnh duy nhất chưa thấy prior art là **handoff giữa cuộc hội thoại mà vẫn giữ prefix y hệt từng byte để prompt cache tiếp tục hit**. Đây là tối ưu chi phí, không phải kết quả khoa học.
- **Tổng thể:** agent always-on tự sửa mình, có supervisor nằm ngoài agent, đã chạy công khai: Ouroboros "Hope" chạy 161 ngày với 1.085 commit tự sửa. Mảng personal agent tự tạo skill (OpenClaw, Hermes) cũng đã đông. **Kiến trúc của INIT không phải moat sản phẩm.**
- **Về stop rule:** prior art chưa làm *trọn bộ* ba claim, nên riêng phần novelty chưa kích hoạt stop rule. Nhưng novelty chỉ ở mức tích hợp, nên giá trị của INIT phụ thuộc hoàn toàn vào bằng chứng của proof sprint. **Khuyến nghị:** chỉ đi tiếp nếu đạt **Stick** (ít nhất 1 thay đổi do agent đề xuất qua được G4) và **Hold** (red team không phá được Root, gog loại mọi mutant). **Improve** được phép ra kết quả null trung thực.
- **Định vị mạnh nhất:** **thư viện mã nguồn mở + technical report**: "Root + gate-of-gates cho mọi vòng self-improvement". Kèm theo bộ mutant M dưới dạng **benchmark cho các gate**, đo false-accept rate của các gate hiện có (LLM review, staged eval, SEAL, PACE). Kết quả benchmark này publish được kể cả khi INIT không cải thiện được gì. Không nên định vị là "agent tự sửa đầu tiên" hay "agent hot-swap đầu tiên".

## Bảng claim

| Claim | Prior art gần nhất | INIT thêm gì | Verdict | Bằng chứng cần có |
|---|---|---|---|---|
| **1. Tự sửa core compiled + hot handoff trong phiên** | **MOSS** (arXiv 2605.22794, 5/2026): agent sửa source của chính nó → build candidate image → replay lỗi trong trial worker → container swap (cần user đồng ý), giữ nguyên volume session → rollback khi health probe fail, LKG lưu riêng. **Ouroboros** (2608.08311): launcher lo release bootstrapping, staging health check, crash rollback. **Gödel Agent**, **Beamcore**: tự sửa logic core ngay trong phiên. **DGM**: candidate bị loại nếu mất khả năng tự sửa code (tiền thân của evolvability probe). **Gated-QD**, **PACE**: gate thống kê có ghép cặp. Claude Code: giữ system prompt cũ khi resume sau upgrade | Handoff **giữa cuộc hội thoại** sang binary agent tự build, prefix y hệt từng byte nên cache vẫn hit; gate cơ học (build, test, self-map drift, boot probe, evolvability probe) thay cho LLM review hay user consent; trạng thái **provisional** chờ gate thống kê quyết; tự rollback | **INCREMENTAL** | 1 thay đổi stick; đo trên N lần handoff: tỉ lệ thành công, độ trễ, cache-read token ở lượt đầu sau handoff so với restart-and-resume, số $ tiết kiệm; fault injection (crash, hang, prefix drift, regression trong lúc probation) → đếm số lần rollback đúng; A/A cho gate provisional |
| **2. Root bất biến tối thiểu + ladder L0–L4** | **Unfireable Safety Kernel** (2606.26057, 6/2026): lớp authorization bằng Rust, tách process, có kill switch, phát "signed evidence", kiểm bằng Z3/Kani; chặn 704/704 lần sửa vào core an toàn. **Ouroboros**: launcher, panic, spend cap nằm ngoài agent; mode Light/Advanced/Pro **do owner chọn**. **AWS graduated autonomy** (8/2026): điểm số tự động → tier T1–T4, giáng cấp ngay lập tức, enforcement ngoài process (cho action, không cho self-modification). **SEAL** (2607.24300): audit niêm phong agent không đọc được, chỉ trả accept/reject. AI control (Redwood), ACL (Korbak 2025), Guillotine. Nhiều thư viện log hash-chain ký Ed25519 | Gom mọi kiểm soát vào một Root nằm ngoài mọi sandbox; ladder điều khiển **phạm vi agent được tự viết lại** (L3 = gate và kernel của chính nó), mở khoá cơ học, tự giáng cấp, không cần người duyệt | **INCREMENTAL** | Red team thật (script + model thật): nâng trần, đọc key, giả lineage, gian lận G4, thoát sandbox, báo cáo số liệu theo lớp tấn công; quỹ đạo ladder với tỉ lệ false-promotion trong A/A; đo kích thước TCB thật |
| **3. Gate-of-gates** | **Oracle assessment** (Jahangirova et al., ISSTA 2016): dùng mutation để tìm false negative, dùng test sinh ra để tìm false positive. **chrisjowen/improve #3** (30/9/2026): mutation-test evaluator với known-good control, "mutant sống sót = lỗi grader". **Who Verifies the Verifier?** (2610.11464, 8/10/2026): verifier tiến hoá cùng agent; check mới chỉ được nhận nếu bắt được cụm lỗi của nó và sạch trên output tốt. **Red Queen GM** (2606.26294): evaluator đồng tiến hoá với agent. ImpossibleRubrics, Recursive Reward Improvement, PROCTOR canary | Thứ bị kiểm là **gate thăng cấp cả hệ** do chính agent viết; mutant là **hệ agent** mang 7 lớp hack đã ghi nhận (log giả, tắt check, canary, answer key, parser fallback, đọc dữ liệu niêm phong), có control C1–C3; bộ mutant, control và reference evaluator do Root giữ; trượt thì bị giáng cấp | **INCREMENTAL** (gần mới nhất) | Kill matrix: mutant × gate (gate đúng, gate bị phá, gate kiểu hệ khác), có false-accept và false-reject; **một họ mutant held-out** viết sau khi M đã đóng băng; ít nhất 1 gate do agent đề xuất chạy qua gog |
| **Tổng thể (personal agent always-on có cả ba)** | Ouroboros Hope (161 ngày, $110,6K), MOSS trên OpenClaw, Hermes, OpenClaw | Tổ hợp đủ ba claim với gate cơ học thay cho LLM review | **INCREMENTAL** | Đường cong cải thiện trên reserve niêm phong (hoặc null trung thực), cộng hai mục ở trên |

## Chi tiết

### Claim 1: live self-modification của core compiled, hot handoff trong phiên

**Prior art gần nhất**
- **MOSS** ([arXiv 2605.22794](https://arxiv.org/html/2605.22794v1), 21/5/2026, USTC/HKUST/HKBU). Đây là hệ gần INIT nhất.
  - Agent sửa source của substrate OpenClaw (routing, hooks, state, dispatch). Mỗi thay đổi là một git commit, có review diff.
  - Host build candidate image rồi chạy trial container, replay lại batch lỗi nhiều lần để lộ flakiness.
  - Promote bằng "in-place container swap" có user đồng ý. Volume user-state (session, memory, credentials) được mount nguyên vẹn sang container mới.
  - Probe window 90 giây; nếu fail thì rollback về bản LKG lưu ở một bản ghi độc lập.
  - Những gì MOSS **không có**: probation sau 90 giây, kiểm định thống kê, bàn về prompt cache, held-out set (cùng 4 task vừa dùng để evolve vừa dùng để đánh giá). Credentials vẫn nằm trong tầm với của agent.
- **Ouroboros** ([arXiv 2608.08311](https://arxiv.org/html/2608.08311)).
  - Launcher "owns startup, process supervision, release bootstrapping, and panic-stop".
  - Có nhắc staging health check và crash rollback nhưng không mô tả cơ chế.
  - Không nói session có sống qua upgrade hay không, không có hot reload, không có cache.
  - Gate là preflight cộng một panel LLM review; không có benchmark gate và không có probation.
- **Tự sửa core ngay trong phiên đã có từ trước:**
  - Gödel Agent monkey-patch chính logic của nó trong lúc chạy ([arXiv 2410.04444](https://arxiv.org/html/2410.04444v4)).
  - Beamcore recompile module của chính nó trong VM đang chạy. README cảnh báo compile hỏng sẽ crash phiên ([GitHub](https://github.com/beamcore/agent)).
  - pi `/reload` nạp extension do agent tự viết vào phiên đang chạy ([pi docs](https://app.unpkg.com/@earendil-works/pi-coding-agent@0.83.0/files/docs/extensions.md)).
- **Cơ chế hệ thống:** hot code loading của Erlang ([docs](https://www.erlang.org/doc/system/code_loading.html)); fork+exec handoff của Cloudflare ecdysis ([blog](https://blog.cloudflare.com/ecdysis-rust-graceful-restarts)); automated canary analysis có rollback tự động (Kayenta, [Netflix](https://netflixtechblog.com/automated-canary-analysis-at-netflix-with-kayenta-3260bc7acc69)).
- **Giữ prefix qua upgrade:** Claude Code chỉ áp upgrade khi launch lại. Khi resume, nó mặc định giữ system prompt cũ để cache không vỡ ([docs](https://code.claude.com/docs/en/prompt-caching)). Cơ chế này do operator điều khiển, không phải tự sửa.
- **Gate thống kê:**
  - Gated-QD (2607.13683): paired 2σ, K = 3.
  - PACE ([arXiv 2606.08106](https://arxiv.org/abs/2606.08106)): e-process, kiểm soát false-commit kể cả khi dừng sớm. Accept tham lam commit 30–42% edit sai.
- **Evolvability probe:** DGM loại candidate mất khả năng sửa code ([arXiv 2505.22954](https://arxiv.org/html/2505.22954)).
- **Không tìm thấy** agent nào thay binary giữa hội thoại mà vẫn giữ phiên. Kết quả gần nhất chỉ là restart-and-resume ([htek.dev](https://htek.dev/articles/copilot-cli-self-restart-extension)) và các feature request. OpenClaw vẫn cắt mọi lượt đang chạy khi restart ([#160269](https://github.com/openclaw/openclaw/issues/160269)).

**INIT thêm gì**
- Handoff giữa hội thoại, cache vẫn hit.
- Gate cơ học (không dùng LLM, không cần user đồng ý), trong đó evolvability probe là phiên bản sống và chặt hơn check của DGM.
- Provisional + gate thống kê + tự rollback.
- "Compiled Rust" chỉ là lựa chọn ngôn ngữ; reviewer sẽ không tính đây là đóng góp.

**Verdict: INCREMENTAL.** Mảnh duy nhất chưa thấy prior art (handoff giữ cache) chỉ có giá trị khi tần suất tự sửa đủ cao để việc rebuild cache tốn tiền thật.

### Claim 2: Root bất biến tối thiểu + autonomy ladder

**Prior art gần nhất**
- **Unfireable Safety Kernel** ([arXiv 2606.26057](https://arxiv.org/abs/2606.26057), 24/6/2026).
  - Lớp authorization viết bằng Rust, tách process, chặn đường tự sửa, có kill switch, "externalized signed evidence".
  - Invariant fail-closed được kiểm hình thức bằng Z3 và Kani.
  - 704/704 lần sửa vào core an toàn bị từ chối; 6.240 round-trip không có bypass.
  - Đây là đối thủ trực tiếp của "Root": nó có formal verification, INIT chưa có.
- **Ouroboros** ([arXiv 2608.08311](https://arxiv.org/html/2608.08311)): launcher, panic, spend cap nằm ngoài agent, governance file được bảo vệ. Mode Light/Advanced/Pro do **owner chọn**, không tự mở khoá theo thành tích. Paper không gọi launcher là bất biến và không dùng log hash-chain.
- **Guillotine** ([arXiv 2504.15499](https://arxiv.org/abs/2504.15499), HotOS 2025): hypervisor cô lập AI, kèm fail-safe vật lý. Một nghiên cứu 2026 về worm AI kết luận: cơ chế bảo vệ nằm trong miền agent kiểm soát thì agent tắt được, nên phải đặt ở miền đặc quyền cao hơn ([arXiv 2606.03811](https://arxiv.org/pdf/2606.03811)).
- **AI control:** trusted monitoring ([Greenblatt et al., 2312.06942](https://arxiv.org/abs/2312.06942)); AI control levels ([Korbak et al., 2504.05259](https://arxiv.org/abs/2504.05259)); RSP/ASL ([Anthropic](https://www.anthropic.com/responsible-scaling-policy)). Đây là thang cấp do người quyết, chiều ngược lại: năng lực càng cao thì safeguard càng nhiều.
- **AWS graduated autonomy** ([blog, 26/8/2026](https://aws.amazon.com/blogs/architecture/closing-the-ai-agent-trust-gap-with-graduated-autonomy/)).
  - Điểm 0–100 tính trên cửa sổ 50 action → tier T1 Probation … T4 Autonomous.
  - Agent mới luôn bắt đầu ở T1. Muốn lên tier thì điểm phải giữ trên ngưỡng suốt cả cửa sổ. Giáng cấp ngay khi vi phạm sàn an toàn.
  - Enforcement nằm ngoài process. Đây là cơ chế ladder của INIT, chỉ khác là áp cho action chứ không cho self-modification.
- **Sealed eval:**
  - SEAL ([arXiv 2607.24300](https://arxiv.org/abs/2607.24300)): audit niêm phong phía harness, agent chỉ nhận accept/reject. Kết luận cần "at least one deployment-acceptance signal outside the agent's control".
  - PROCTOR: holdout đóng băng, canary.
  - Gated-QD: một lần so sánh niêm phong trên reserve.
- **Lineage ký và hash-chain:** đã có nhiều thư viện 2026 (CSAE, Maatora, agentproofs, halo-record, RootSign; xem notes).
- **Lý thuyết:** "The Self-Improvement Problem" ([arXiv 2610.21646](https://arxiv.org/html/2610.21646), 15/10/2026) chứng minh không gate tự động nào vừa sound vừa complete, và khuyên nên có grounding bên ngoài hoặc do con người.

**INIT thêm gì**
- Một Root duy nhất giữ đủ stop, trần tiền, key, lineage ký, S niêm phong, bộ mutation và sealed audit.
- Ladder quyết định **agent được tự viết lại phần nào của chính nó**. Tổ hợp này không tìm thấy ở đâu.

**Điểm yếu reviewer sẽ chỉ ra**
- Chữ "minimal" bị lung lay: thiết kế ước 1.000–1.500 dòng, nhưng ngân sách build là khoảng 7.500 dòng (ADR-007).
- Chưa có formal verification.
- Mệnh đề "không cần người duyệt" va vào định lý trên. Câu trả lời của INIT là Root do người giữ cộng sealed task, và nên nói rõ điều đó.

**Verdict: INCREMENTAL.**

### Claim 3: gate-of-gates

**Prior art gần nhất**
- **Oracle assessment** ([Jahangirova et al., ISSTA 2016](https://discovery-pp.ucl.ac.uk/id/eprint/1493269)): mutation tìm false negative của oracle, test sinh ra tìm false positive. Đây là gốc ý tưởng.
- **chrisjowen/improve #3** ([GitHub, 30/9/2026](https://github.com/chrisjowen/improve/issues/3)).
  - Mỗi evaluator đi kèm known-good control và các biến thể sai; mutant nào còn sống là lỗi của grader.
  - Tự ghi rõ giới hạn: chỉ kiểm độ nhạy với lỗi đã khai báo.
  - Evaluator ở đây là tĩnh, không do agent viết.
- **Who Verifies the Verifier?** ([arXiv 2610.11464](https://arxiv.org/html/2610.11464v1), 8/10/2026, AWS).
  - Verifier tiến hoá cùng agent. Check mới chỉ được nhận nếu bắt ≥ ½ cụm lỗi của nó và sạch trên output tốt. Có locked test set.
  - Phát hiện quan trọng: bỏ guard thì verifier thoái hoá thành gần như pass-all mà điểm task vẫn như cũ. Nghĩa là điểm task không chứng nhận được verifier.
  - Không có bộ cố định các ứng viên gian lận, không có kill switch, không có ladder.
- **Red Queen Gödel Machine** ([arXiv 2606.26294](https://arxiv.org/abs/2606.26294)): evaluator đồng tiến hoá với agent, neo vào điểm IMO do người chấm. Trang abstract không mô tả meta-check kiểu "loại bad, nhận good". Tôi chưa đọc full text.
- **Kiểm rubric bằng tấn công:**
  - ImpossibleRubrics ([2609.16816](https://arxiv.org/abs/2609.16816)): rubric sinh ra bị khai thác 8–26% số lần.
  - Recursive Reward Improvement ([2610.21040](https://arxiv.org/html/2610.21040)).
  - Harness tampering audit ([2609.00069](https://arxiv.org/abs/2609.00069)): gieo cặp edit tampered/benign.
  - PROCTOR canary ([2609.02246](https://arxiv.org/abs/2609.02246)).

**INIT thêm gì**
- Đối tượng bị kiểm là gate thăng cấp do agent viết.
- Mutant là hệ agent hoàn chỉnh mang 7 lớp hack có thật (DGM, STOP, PROCTOR), cùng control C1–C3 để gate "reject hết" cũng trượt.
- Bộ mutant, control và reference evaluator do Root giữ trên sealed lane; trượt thì bị giáng cấp.
- Từng mảnh đều đã có; tổ hợp thì không tìm thấy.

**Verdict: INCREMENTAL, gần mới nhất.** Hai giới hạn lớn:
- Định lý 1 của Moser et al. cho thấy gog chỉ chứng minh được độ nhạy với lỗi đã khai báo.
- INIT chưa có gate nào do agent viết (chưa tới L3), nên cơ chế này chưa từng được chạy thật.

### Bằng chứng cần có, gộp theo proof sprint

| Mục tiêu sprint | Bằng chứng tối thiểu để claim đáng tin |
|---|---|
| Stick | ≥ 1 thay đổi do agent tự đề xuất qua G4 v2 trên S v1 và thành LKG, trên deepseek-flash thật |
| Improve | Hiệu số ghép cặp trên reserve chưa từng chạm, có CI và A/A calibration; null trung thực cũng chấp nhận được. Lưu ý: với khoảng 30 task, MDE chừng 12–20 điểm, nên chỉ phát hiện được mức tăng lớn |
| Hold | Báo cáo red team theo lớp tấn công, kèm số lần thử và số lần thành công, cho cả adversary dạng script lẫn model thật |
| Gate-of-gates | Kill matrix mutant × gate; một họ mutant held-out; false-reject rate trên gate tốt |
| Hot handoff | Tỉ lệ cache-hit sau handoff so với restart-and-resume; số rollback đúng khi fault injection |

### Định vị và venue

- **Nên làm:**
  - Một technical report trên arXiv cộng thư viện "Root + gog" gắn được vào vòng self-improvement khác (DGM-style, OpenEvolve, Harness², Gated-QD).
  - Một **benchmark mutant cho các self-improvement gate**, đo false-accept của LLM review, staged eval, SEAL và PACE trên 7 lớp hack. Đây là đóng góp rõ ràng nhất có thể đạt với ngân sách $3–5.
- **Cộng đồng phù hợp:**
  - Workshop NeurIPS/ICLR 2027 về agents, lifelong learning, safety/reliability.
  - Cộng đồng AI control (Redwood, UK AISI, Alignment Forum), vì Root cộng red team khớp khung "control evaluation".
  - Venue kiểm thử phần mềm (ICST Mutation workshop, ISSTA/FSE new-ideas), vì gog là mutation analysis của acceptance oracle.
  - Main track chỉ khả thi khi có đường cong cải thiện **và** kết quả red team.
- **Không nên:** pitch sản phẩm bằng kiến trúc. Người dùng personal agent không mua "gate-of-gates". Giá trị sản phẩm phải đến từ chỗ agent tốt dần lên thấy được, mà điều đó chưa được chứng minh.

### Giới hạn của bản rà soát

- Khoảng 20 truy vấn; không tìm nguồn tiếng Trung hay sản phẩm đóng. Lượng preprint 2026 trong mảng này rất lớn, nên có thể xuất hiện bản gần trùng bất cứ lúc nào.
- Chưa đọc full text của Red Queen GM, Unfireable Safety Kernel, 2609.26457 (RSI cho research agent) và 2609.17817 (benchmark nhiễm độc cho agent tự sửa).
- Venue fit là nhận định của tôi, không có nguồn.
