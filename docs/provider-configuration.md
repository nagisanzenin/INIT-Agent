# Configure an API gateway or local model

> **Status: experimental (v0.2.0-preview).** Live-tested: DeepSeek through the installer
> (deepseek-flash, deepseek-v4-pro). OpenAI's gpt-6.1-sol on OpenAI's own host was qualified live
> on v0.5.2 (`scripts/qualify-provider.sh` PASS 7/7 on a v0.5.2 install, 2026-10-29:
> build/status/qualify/openai-gpt-6.1-sol-v0.5.2.json); v0.5.3 changed how a cut request is booked
> and has not been re-qualified yet, although the app still labels it live-tested. Every other
> configuration below
> (Chat-compatible gateways, OpenAI's gpt-6-astra, Responses or Messages at a gateway's URL,
> Anthropic, local no-auth servers) is implemented and covered by offline tests only. Set them
> up in the app (`init-agent`'s first run, `/setup`, `/model`, `/key`) or with
> `init-root providers add`; the app marks them "experimental (not live-tested)".

## In the app (v0.2.0-preview, UX-KEYMODEL)

You do not need to edit the two files below by hand. On the first `init-agent` without a usable key, the TUI asks
for the provider, then the key (hidden; it goes only to the Root, stored 0600), then the model, and starts the
chat. Later, `/model` switches models, `/key` replaces a key and `/setup` runs the setup again. From a shell:

```
init-root providers list                     # configured providers and models, and the catalog
init-root models [--provider P]
init-root providers add gateway --base-url https://gw.example/team/v1 --model custom-model --price 1.00,1.00,2.00
init-root keys set gateway                   # hidden input; never an argument
init-root model use gateway/custom-model     # rewrites [model] in kernel/config.toml; the kernel restarts
```

`providers add` writes the same values as the fragments below: the charter's `[actions.providers]` (µUSD per
1M tokens) through the charter check and apply path, and the kernel's `[providers.<id>]` table (decimal USD per
1M tokens). Presets: `deepseek` (live-tested), `openai` (gpt-6.1-sol qualified live on v0.5.2, not yet re-qualified
on v0.5.3; gpt-6-astra experimental), `gateway`, `anthropic`, `local` (experimental, not live-tested). `anthropic`
and `openai` with `--base-url` configure a Messages or Responses gateway, which is experimental: the live test was of
OpenAI's own host.

Implementation guide, 2026-10-23. These examples describe configurable wire compatibility;
they are not live certification of a vendor or model. Native isolation and the release A/B
qualification are tracked separately.

Two configurations must agree. The owner’s Root charter authorizes the destination,
endpoint, authentication and tariff. `INIT_HOME/kernel/config.toml` selects the model,
body adapter and proxy routes. A model descriptor alone grants no network access.
Use the exact same provider alias and model ID in both configurations. A model ID is
sent upstream unchanged; it must exist on that server.

## Chat-compatible gateway with a path prefix

Merge this fragment into the existing Root charter. The host and tariff below are
illustrative, not a real service or a current price. Replace them with your gateway
and its effective tariff before making requests. Root prices use **micro-USD per
million tokens**: `1000000` means USD 1 per million tokens.

```toml
[actions.providers]
destinations = ["gateway.example:443"]
methods = ["POST"]
paths = ["chat/completions"]

[actions.providers.providers.custom]
host = "gateway.example:443"
api = "chat"
base_path = "/team/v1"
auth = "bearer"

[actions.providers.models."custom-model"]
min_level = 0
input_musd_per_mtok = 1000000
cache_read_musd_per_mtok = 1000000
output_musd_per_mtok = 2000000
reserve_output_tokens = 4096
```

Merge this matching fragment into `INIT_HOME/kernel/config.toml`. Kernel proxy tariffs
use decimal **USD per million tokens**, so they use different units from the Root.
The limits here are examples; set the model’s actual context/output limits.

```toml
[model]
provider = "custom"
id = "custom-model"

[model.descriptor]
api = "chat_completions"
context_window = 131072
max_output = 32768
default_max_tokens = 4096
request_max_tokens = 4096
chat_stream_usage = true
native_extensions = false

[providers.custom]
preset = "deepseek"

[providers.custom.models."custom-model"]
context_window = 131072
max_output = 32768
reserve_output_tokens = 4096
prices.standard = { input = "1.00", cache_read = "1.00", output = "2.00" }
```

The body requests `/custom/chat/completions`; Root forwards to
`https://gateway.example/team/v1/chat/completions`. `preset="deepseek"` supplies the
canonical Chat route here; the custom alias does **not** enable DeepSeek thinking
fields. For the selected model, a descriptor also infers the matching preset when
`preset` is omitted. Explicit `preset="openai"` with a Chat descriptor adds the
canonical Chat route to the kernel allowlist; Root must still allow it independently.

Place the gateway credential in `ROOT_HOME/keys/providers/custom`, owned by the Root
owner with mode `0600`. It is a secret file, not a kernel configuration value. Root
injects the authorization header and does not forward body-supplied credentials.
Do not put keys in the model descriptor or repository.

Changing `destinations` replaces that list. To retain existing providers, retain their
exact destinations, endpoint paths, provider tables and model tariff entries too.
These are fragments, not complete replacement charter/kernel files.

## Native Responses and Messages aliases

Use the same two fragments and substitute the following fields together. Both native
adapters emit `v1/` in the internal request path, unlike the Chat adapter.

| Setting | Responses gateway | Messages gateway |
|---|---|---|
| Root provider `api` | `"responses"` | `"messages"` |
| Root class `paths` | `["v1/responses"]` | `["v1/messages"]` |
| Root provider `base_path`, normal upstream | `""` | `""` |
| Root provider `base_path`, upstream `/team/v1/...` | `"/team"` | `"/team"` |
| Kernel descriptor `api` | `"responses"` | `"messages"` |
| Kernel provider `preset` | `"openai"` | `"anthropic"` |
| Typical Root `auth`, if required by that gateway | `"bearer"` | `"x-api-key"` |
| Body request | `/custom/v1/responses` | `/custom/v1/messages` |
| Output bound on the wire | `max_output_tokens` | `max_tokens` |

Choose authentication according to the actual gateway; the API dialect does not
imply a credential scheme. Set `base_path=""` for the normal native `/v1` upstream;
using `"/v1"` here would produce `/v1/v1/...`. Prefixes are joined literally.

These select the existing native adapters, including native streaming/tool-call
parsing and provider-bound replay. They do not translate native requests into Chat.
The configured alias remains the provider identity in generated conversation records
and Root request records.

Unknown custom models default to text/tool operation with `images=false`. Their
model-specific cache/thinking request extensions default off. Set
`native_extensions=true` only for an endpoint/model supporting the existing adapter’s
extensions: Responses reasoning/encrypted-content requests and explicit cache fields,
or Messages adaptive thinking, output effort, cache controls and thinking-binding beta.
It is an explicit capability choice, not automatic capability detection. Known built-in
models retain their existing defaults; an explicit descriptor can override this flag.
The current descriptor does not expose a generic vision capability switch.

## Local Chat server without authentication

Starting from the Chat example, replace the Root destination and provider table with:

```toml
[actions.providers]
destinations = ["127.0.0.1:11434"]
methods = ["POST"]
paths = ["chat/completions"]

[actions.providers.providers.custom]
host = "127.0.0.1:11434"
api = "chat"
base_path = "/v1"
local = true
auth = "none"
```

Keep the matching model tariff table. For a server with no API charge, set all Root
and kernel tariff values to zero; token usage is still recorded. This excludes hardware,
electricity and hosting costs. Use the server’s exact loaded model ID in both files.
The forwarded URL is `http://127.0.0.1:11434/v1/chat/completions`. The local server
must run in the network environment visible to Root; a host outside Root’s container
is not automatically reachable as localhost.

No key file is read for this provider. Unauthenticated access requires explicit
`local=true`, an explicit API dialect and an exact `127.0.0.1:<port>` or
`localhost:<port>` destination. LAN addresses, IPv6 loopback and arbitrary unauthenticated
remote hosts are not covered by this configuration. This can use an Ollama-compatible
server’s **Chat-compatible endpoint**; it does not implement Ollama’s native API.

## Usage options and cost interpretation

`chat_stream_usage=true` requests the standard final streamed usage chunk. Set it to
`false` if a compatible server rejects `stream_options`; the field is then omitted.
If the server supplies no usage, Root records an estimate and marks token totals
incomplete. Turning the option off does not turn unknown usage into zero usage.
The flag applies to generic Chat aliases; native Messages/Responses use their own
stream usage events and built-in DeepSeek behavior is unchanged.

`request_max_tokens` caps every request, including compaction and retries.
`default_max_tokens` is the normal request target, bounded by that cap.
`max_output` and `context_window` describe the model. These values must be positive,
consistent, and accepted by the upstream server. One body process selects one descriptor;
changing the selected descriptor requires a body restart.

Root logs the configured provider alias on the start record; its matching end record
contains usage/cost under the same request ID. Native Messages input totals include
uncached input, cache reads and cache creation; Responses/Chat include the provider’s
total input count. Do not add cache-read counts to those totals again.

Costs are tariff-based estimates, not invoices. Root tariffs are shared by model ID
within an action class, so two gateways charging differently for the same model ID
cannot have different Root tariffs in one class. Separate model classes can isolate
live tariffs, but sealed lanes currently accept only the `providers` class, so this is
not a complete solution for provider-specific sealed pricing. Root prices cache writes
at the model's `cache_write_musd_per_mtok` (5-minute or single-class writes) and
`cache_write_1h_musd_per_mtok`; `providers add` writes the catalog's write prices, and a
model without them bills writes at twice its input price, so the Root never under-counts
(v0.5.0). Kernel proxy schedules/tiers give the kernel's own estimate, which no
owner-facing figure shows since v0.5.3: the Root's ledger prices every request (DeepSeek at
its peak price around the clock, an upper bound). Root settled-live totals exclude
sealed/twin totals, which stay in their own ledgers.

**Price drift (v0.5.3).** When a provider reports a usage class the charter has no price
for (a new cache TTL, audio tokens on a gateway), the Root books those tokens at the
model's dearest price, at most the request's reservation and never at 0, flags the
request `price_drift` (its `end` record lists the fields under `drift`) and raises an
alert naming the provider, model and fields. The charter cannot price a field the Root's
usage reader does not know: a Root release that reads it prices it exactly, and until then
the dearest price is its bound. A stream cut before its final usage (Anthropic's `message_start`
carries only the input) is booked with its output at the request's `max_tokens`. Product USD fields are observe-only; manual stop remains available.
External benchmark spending caps are enforced by the benchmark harness separately.

## Compatibility boundaries

Implemented configuration covers Chat Completions, Responses and Messages dialects
with exact destinations and bounded output, native stream/tool parsing and usage taps.
Credentials support bearer tokens, `x-api-key`, or explicit local no-auth. Remote
connections use HTTPS; local loopback uses HTTP. Redirects and ambient `HTTP_PROXY` /
`HTTPS_PROXY` settings are disabled: configure an API gateway as the destination itself.

Not implemented by these examples: native Gemini or native Ollama codecs; AWS SigV4 /
Bedrock; Azure-specific `api-key` authentication and deployment/query contracts;
arbitrary auth headers, OAuth token refresh, mTLS/custom trust configuration; WebSockets;
automatic model discovery/qualification; generic vision; or guaranteed compatibility
with every model’s reasoning extensions. A gateway may translate some of these services
into a supported dialect, but that gateway must be independently configured and tested.
Production test mode is separate: it refuses remote endpoints and requires test-prefixed
credentials for authenticated loopback fixtures. Local production inference does not
require enabling test mode.

Offline regression tests cover descriptor validation/options, native alias identity and
payloads, canonical routes, tool/usage stream fixtures, and Root local no-auth admission.
They do not replace live endpoint qualification or the multi-OS release tests.
