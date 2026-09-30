# A2Agent configuration example

Use A2Agent's hosted API through TinyHarness's existing `--openai-compat`
provider. No additional provider or dependency is needed. This is an optional
remote backend: prompts, supplied workspace context, and tool results are sent
to the service, and API usage may incur charges.

## Start TinyHarness

Install TinyHarness using the [installation guide](../../../README.md#installation),
and obtain an A2Agent API key. Set `OPENAI_API_KEY` to that key, not an OpenAI key.
The values below are placeholders; do not commit real credentials or paste them
into bug reports. Prefer injecting the variable through your secret manager.

Bash:

```bash
export OPENAI_API_KEY='YOUR_A2AGENT_API_KEY'
tinyharness --openai-compat --url https://api.a2agent.me --skip-health-check
```

PowerShell:

```powershell
$env:OPENAI_API_KEY = 'YOUR_A2AGENT_API_KEY'
tinyharness --openai-compat --url https://api.a2agent.me --skip-health-check
```

TinyHarness adds `/v1/models` for model discovery and `/v1/chat/completions`
for streaming chat, and sends `Authorization: Bearer <key>`. These routes and
the base URL are documented in [A2Agent's API reference](https://a2agent.me/llms.txt).

`--skip-health-check` skips TinyHarness's separate `/health` probe, which is not
part of the documented A2Agent API. It does **not** skip authentication or model
discovery, validate credentials, or prove that inference works. Without this
flag, a failed health check is a warning rather than a startup blocker.

The environment variable is read at launch and is not saved to settings.
In contrast, `--api-key <key>` saves the key in the local settings JSON.
TinyHarness does not read an `A2AGENT_API_KEY` variable directly.

## Select a model before sending a prompt

Inside TinyHarness, list models, then replace `YOUR_MODEL_ID` with an exact ID
returned for your key:

```text
/model
/model YOUR_MODEL_ID
```

TinyHarness may automatically select the first available model at startup if
there is no valid saved selection. Check the selected model before sending a
prompt. `/model <id>` saves the selection but also accepts unlisted IDs, so a
successful selection message is not confirmation that the API accepts the ID.

For an independent model-list check in Bash:

```bash
curl --fail-with-body https://api.a2agent.me/v1/models \
  -H "Authorization: Bearer $OPENAI_API_KEY"
```

## Optional saved configuration

With TinyHarness closed, merge these fields into
`~/.config/tinyharness/settings.json`, preserving unrelated settings and entries
for other providers. Replace `YOUR_MODEL_ID` with your chosen ID and continue
supplying the key through `OPENAI_API_KEY`.

```json
{
  "last_provider": "OpenAiCompat",
  "provider_urls": {
    "OpenAiCompat": "https://api.a2agent.me"
  },
  "provider_models": {
    "OpenAiCompat": "YOUR_MODEL_ID"
  },
  "skip_health_check": true
}
```

Then launch `tinyharness` without provider flags. URL and model settings are
stored per provider kind, not per gateway; switching to another OpenAI-compatible
service shares the same entries. The saved `skip_health_check` setting is global;
omit it and pass the CLI flag per launch if other providers should keep their
health checks.

## Verify and troubleshoot

This example has not been live-tested against A2Agent. Model listing alone does
not verify streaming or tool calling. In a disposable workspace, first try a
short chat prompt, then test a read-only tool task with a model that supports
tool calling before using it for coding work.

- **401/403:** verify the A2Agent key and its access. An explicit `--api-key`
  overrides `OPENAI_API_KEY`, which overrides a saved key.
- **Empty model list or unknown model:** check `/v1/models` directly; discovery
  failures can appear as an empty list. Confirm the exact ID and key permissions.
- **Timeouts:** inspect the service error; if a longer response timeout is needed,
  use `/timeout 120` (seconds). `/timeout 0` restores the provider default.
- **Tool errors:** verify the selected model and route support the required
  features. This configuration does not imply support for every catalog model.
- **Web tools:** TinyHarness's `web_search` and `web_fetch` use Ollama's web API
  and need a separate Ollama key; an A2Agent key does not configure those tools.

See the [configuration guide](../../configuration.md#openai-compatible-provider-settings)
for general provider settings.
