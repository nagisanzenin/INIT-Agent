# INIT v1.0 program: a release that real users can use (started 2026-10-20 21:40)

## Owner mandate (verbatim intent)
"Keep going until I have a final product release ready for users":
1. Excellent UI/UX from install to daily use. The current setup is clunky.
2. Actually achieve INIT's vision: self-evolution that sticks, an honest measuring stick, the autonomy ladder.
3. Support every current LLM provider API, proxy/gateway, local LLM, and every subscription plan other agentic coders support (where the terms allow it), with every model and price current for up to one year from release.
4. Cross-platform releases: Linux, macOS and Windows (owner, 21:55).
5. Budget: $5 on DeepSeek (the existing key) for work and benchmark experiments. This is a hard cap, Root-metered plus the real balance check.
Standing rules still apply: proposals, not menus; parallel lanes; progress % and ETA; disk ≥ 5 GB floor; never handle keys or credentials in chat; no cloud spend beyond the $5.

## Phases
| Phase | What | Status |
|---|---|---|
| A | ADR-009 Codex-style permissions (SBX-009, WIRE-009, DOC-009) → v0.1.1 | running |
| B | Research ×6 (incl. cross-platform: Linux bwrap/Landlock/seccomp, Windows AppContainer/Job Objects, services, snapshots, signing, CI): API providers + pricing; proxies/gateways/local; subscriptions + ToS; install/daily UX; M7 task sources | running (notes in research_notes/INIT v1 providers UX vision/) |
| C | Design docs from the research: providers v2 (registry, auth kinds, model catalog with prices owned by the Root (never agent-writable; price increases apply automatically, decreases need the owner)), onboarding/UX spec, M7 design | after B |
| D | Build lanes: provider adapters (OpenAI Chat+Responses, Anthropic, Gemini, OpenAI-compatible generic, Bedrock/Vertex/Azure), Root egress classes + local-loopback class, subscription auth (ToS-allowed only; credentials stay in the Root), model catalog + pricing, install UX (prebuilt body, one-line installer, first-run wizard in the TUI), M7 (sealed S ≥ 30 cheap tasks, n=30 R=3 calibration, L1 task families) | after C |
| E | Real-model validation within $5; M7 calibration; benchmark experiment | after D |
| P | Platforms: Linux sandbox + systemd + reflink/copy undo; Windows sandbox + per-user service + named pipes/AF_UNIX + Job Objects; cross-compile CI matrix; installers (brew/curl|sh/winget/scoop) | after C, parallel with D |
| F | Public release prep: license (owner picks), signing/notarization decision (costs), second security review, history scrub | last |

## Decisions pending the owner (ask near the release, with a picker)
- License: recommend Apache-2.0.
- Making the repo public, and when.
- Code signing: Apple Developer ID ($99/yr) and Windows signing (paid) are optional; without them, users see Gatekeeper/SmartScreen warnings.
- GitHub Actions minutes for Windows/macOS runners on a private repo (2×/10× multipliers): stay within the free tier, or the owner authorizes.
