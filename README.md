<p align="center">
  <img src="assets/gray-logo.svg" alt="gray" width="96">
</p>
<h1 align="center">gray-grok-sub</h1>
<p align="center">Run gray turns on Grok — an xAI / Grok Build provider sidecar.</p>

Relays gray chat turns through the Grok Build CLI's `grok agent stdio` ACP subprocess.
This matches t3code's `GrokDriver`.

## Authentication

Two methods, resolved in this order:

1. **`xai.api_key`** — set `XAI_API_KEY`. Used when present.
2. **`cached_token`** — run `grok login`; the CLI's cached login is used otherwise.

If neither is available, `/grok` and provider calls return an `unavailable` error with the
setup hint.

## Setup

1. Install the Grok Build CLI (`grok`) and sign in with `grok login`, or export `XAI_API_KEY`.
2. Run `/grok` to check status.

## Environment

| Variable | Default | Meaning |
| --- | --- | --- |
| `GRAY_GROK_SUB_COMMAND` | `grok` on `PATH` | Binary override |
| `GRAY_GROK_SUB_PERMISSION` | unset | `approval-required` passes `--permission-mode default`; unset uses the CLI default |

The sidecar sets `GROK_OAUTH2_REFERRER=gray` on the spawned CLI.

## License

MIT — see [LICENSE](LICENSE).
