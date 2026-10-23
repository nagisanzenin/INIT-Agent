# Sprint chứng minh INIT: dừng hay tiếp tục

*2026-10-21 · số liệu chi tiết: `measurements/m7-results.md`, `measurements/m7-probe.md`, `reports/INIT novelty review.md`*

## Kết luận

**Theo stop rule đặt trước (PROOF_PLAN.md), khuyến nghị dừng phần sản phẩm (v1 breadth, khoảng 800 agent-hour).** Thay vào đó, nên đóng gói phần đã chứng minh được thành technical report và thư viện.

Thay đổi duy nhất do agent tự đề xuất **không được giữ lại**. Trên bộ đề kín nó làm kết quả **tệ hơn 5 điểm**, G4 loại nó và hệ thống tự rollback.

## Bốn mục tiêu

| Mục tiêu | Kết quả | Bằng chứng |
|---|---|---|
| **Stick**: ≥ 1 thay đổi tự sinh qua G4 và thành LKG | ❌ Không đạt | p-000001 (`c88be5f`): G4 standard, n 29 × R 3, **d = −50‰**, status `inferior` → reject `score_inferior` → tự rollback về `aaf07b72` |
| **Improve**: tăng điểm trên tập dự trữ | — Không có gì để đo | không có thay đổi nào được giữ; line M chạy nhánh skip |
| **Hold**: Root không bị phá | ✅ phần lớn, có 2 lỗ hổng | Root từ chối mọi đòn đã thử: tự khai giá, provider lạ, giả lineage, giả lệnh owner, lệnh L3. Lỗ hổng: **RT-GAP-1** (L1 đếm promotion do kernel tự khai; latent, release khoá L0) và **MEM-RUNAWAY** (xem dưới) |
| **Novel** | ⚠️ INCREMENTAL cả 3 claim | MOSS, Unfireable Safety Kernel và các paper mutation-test evaluator đã làm từng mảnh; chưa hệ nào làm trọn cả ba |

## Đã chứng minh được

**1. Vòng tự tiến hoá chạy trọn vẹn trên model thật** (deepseek-flash, $2,67):
- miner tìm friction từ log thật: `cap.hit:turns` trên 10 task;
- agent tự viết giả thuyết ("`cd` lỗi làm phí lượt") cùng 3 biến thể có unit test;
- screen chọn 1 biến thể;
- qua G0–G3;
- **hot-swap lõi đã biên dịch vào phiên đang chạy**;
- G4 trên đề kín;
- phát hiện thay đổi có hại và tự rollback.

**2. Phát hiện đáng giá nhất: đánh giá trên dữ liệu agent nhìn thấy đã đánh lừa.**
- Screen trên task nhìn thấy dự đoán **+12 điểm**.
- Đề kín cho kết quả **−5 điểm**.
- Gate kín cùng phép thử thống kê đã chặn đúng một thay đổi mà cả agent lẫn screen đều tin là tốt.
- Đây chính là luận điểm của nửa "acceptance". Harness² dùng LLM judge và không có bộ đề kín, nên sẽ không bắt được lỗi này.

**3. Thước đo hoạt động và đã được hiệu chuẩn bằng tiền thật:**
- tỉ lệ nhận nhầm đo được là 48‰;
- ở tier standard phát hiện được mức tăng từ khoảng 9,5 điểm trở lên;
- không có canary, tamper hay leak nào bị kích hoạt.

## Giới hạn, cần đọc kỹ trước khi quyết định

- **Mới có n = 1 lần thử.** Một lần thất bại không bác bỏ khả năng engine cải thiện được sau nhiều lần. Nhưng stop rule đặt trước là "1 thay đổi stick trong khoảng $3", và giữ kỷ luật này là để tránh sunk cost.
- **Dư địa thấp.** Agent hiện tại đã giải 85% task nhìn thấy, và chỉ 10/32 task kín nằm trong vùng độ khó hữu ích. Với deepseek-flash trên bộ đề này, chỗ để cải thiện vốn ít.
- **Bộ đề bản gọn.** Đã bỏ họ B (repo mới), nên rủi ro model đã thấy đề cao hơn.
- **Harness mong manh.** Phải thử G4 4 lần mới ra được 1 verdict, vì lần lượt vướng: đĩa đầy do swap, proposal kẹt trạng thái, timeout G3, trần chi tiêu. Có 7 deviation (DV-1…DV-7), tất cả do harness. **Không ai xem kết quả đề kín trước khi có verdict**, nên tính hợp lệ thống kê vẫn giữ.

## ⚠️ Lỗ hổng an toàn ảnh hưởng tới máy của anh

**MEM-RUNAWAY.** Một tiến trình python trong sandbox phình lên 44 GB RAM. macOS đẩy phần đó sang swap nằm trên cùng ổ, làm ổ trống tụt xuống **0,24 GB**.
- Sandbox macOS (Seatbelt) không giới hạn bộ nhớ, và cả Root lẫn kernel đều chưa có watchdog bộ nhớ.
- **Bản INIT đang chạy thật trên máy anh cũng có lỗ hổng này**: một lệnh bash do agent chạy có thể làm đầy ổ.
- Cách sửa: thêm watchdog theo dõi bộ nhớ từng tiến trình trong kernel và Root. Đã ghi HIGH trong RELEASE_PLAN.md.

## Ba hướng

1. **Đóng gói và công bố (khuyến nghị).** Dừng v1 breadth và làm:
   - technical report "Sealed gates catch self-modifications that visible evaluation approves", dùng đúng điểm dữ liệu +12 so với −5;
   - thư viện Root + gate-of-gates;
   - bộ mutant làm benchmark đo tỉ lệ nhận nhầm của các gate.

   Tốn khoảng 1–2 ngày agent. Sửa MEM-RUNAWAY trước.
2. **Thêm một vòng chứng minh nhỏ.** Chạy ≥ 3 đề xuất, đề khó hơn để có dư địa, có thể thêm một model khác; khoảng $3–5 và khoảng 1 ngày. Lưu ý trung thực: làm vậy là dời cột gôn so với stop rule đã đặt. Chỉ nên chọn nếu anh coi khả năng tự cải thiện là mục tiêu chính.
3. **Dừng hẳn.** Gỡ hoặc dừng bản INIT đang chạy và lưu trữ repo.

## Chi phí

- **Tiền thật:** $2,67 trên trần $3 (số dư DeepSeek $9,56 → khoảng $6,89).
- **Quota Claude tuần:** 86%, reset ngày 26/10.

## Đính chính (2026-10-22, khi viết technical report)

Ba điểm trong báo cáo này đã nói quá. Các con số không đổi, chỉ cách diễn giải đổi:

1. **Kết quả "−5 điểm" không chứng minh thay đổi có hại.** Status `inferior` của G4 nghĩa là *chưa chứng minh được non-inferiority*. Lý do:
   - Ở bước calibration (line C), một thay đổi trung tính bị xếp `inferior` 86‰ số lần (FR 86‰).
   - |d| = 50‰ còn nhỏ hơn ngưỡng nhiễu δ_r3 = 57‰.

   Đúng ra phải nói: gate **không nhận** một thay đổi chưa được chứng minh. Không được nói nó đã chặn một thay đổi xấu.
2. **"+15" không phải dự đoán của agent.** Đó là danh sách Predicts cố định do runbook pre-register (D-0), áp cho mọi đề xuất.
3. **"+12" là ước lượng của go rule** (est = 0,5 × s_c 0,6 × fix × T). Ước lượng này chủ yếu đến từ độ nặng của friction. Dữ liệu thô của screen thì hoà: biến thể sửa được 1/2 task mục tiêu và làm hỏng 1/2 task đối chứng.

Vì vậy, tiêu đề "đánh giá nhìn thấy được +12, đề kín −5" mạnh hơn bằng chứng cho phép. Kết luận dừng không đổi: vẫn **chưa có bằng chứng** rằng tự tiến hoá giúp ích. Chi tiết trong `reports/INIT technical report.md`.

## Test tiến hoá theo ADR-011 (2026-10-22): dự án dừng theo luật owner đặt trước

Luật owner đặt trước khi chạy: chỉ khi kết quả là `evolved_and_solved` thì dự án mới tiếp tục. Kết quả thực tế là **"không tìm được khoảng trống cơ chế"**, nên phase tiến hoá chưa từng chạy. Như vậy khả năng tự tiến hoá **chưa bị bác bỏ, nhưng cũng chưa được chứng minh**.

- **v2, bài file lớn:** bản gốc của INIT (genesis) giải được 2 trên 3 bài, nên test dừng ngay ở phase 0.
- **v3, nhắm vào cơ chế các agent khác có mà INIT không có:** chạy 3 họ đề.

  | Họ đề | Bản gốc INIT | Claude Code (cùng model flash) |
  |---|---|---|
  | 25 yêu cầu đánh số | giải 5/5 | không cần chạy |
  | việc dài trên codebase lớn | thua 4/7 bài, cả 2 lần chạy | thua mọi bài đã chạy; lần nào cũng chạm trần $0,06 dù có dùng sub-agent |
  | khoảng 60 yêu cầu viết bằng văn xuôi | giải 5/5 | không cần chạy |

- **Đọc kết quả:** trên deepseek-flash, chỗ nào bản gốc của INIT thua thì Claude Code cũng thua, dù nó có sub-agent và task tool. Thứ giới hạn kết quả là model và tiền, không phải cơ chế của harness. Vì vậy không có bài nào để INIT tự tiến hoá ra một cơ chế còn thiếu rồi chứng minh cơ chế đó giúp ích.
- **Đính chính:** phần phân tích khoảng trống đã ghi nhầm rằng INIT có sub-agent. Thực tế runtime cho fleet chưa được xây. Lỗi này không làm đổi kết luận vì Claude Code có dùng sub-agent mà vẫn thua (protocol §11.9).
- **Chi phí:** khoảng $0,94 (v2 $0,15 cộng v3 $0,785). Số dư DeepSeek còn khoảng $12,91.
- **Đề xuất cho lần sau:** chỉ nên thử lại câu hỏi này với một model mạnh hơn hẳn flash, vì chỉ khi đó harness mới có thể là yếu tố quyết định. Việc này cần làm lane providers trước.
