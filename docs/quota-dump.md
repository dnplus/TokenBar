# Dump quota windows on macOS

`tokenbar-quota` prints one JSON snapshot of every quota window TokenBar can read on this Mac. It does not open Syrtis. Build and run it on the Mac where Claude, Codex, Grok, Antigravity, and Cursor are signed in. A Linux build can compile the same binary, but Keychain, the Antigravity language server, and `state.vscdb` are on that Mac.

## Build

From the repository root, on Apple Silicon:

```sh
CARGO_PROFILE_RELEASE_STRIP=none cargo build --release -p tb_core_ffi --bin tokenbar-quota
```

`make quota-dump` runs that command. The binary is `target/release/tokenbar-quota`. Swift and the Syrtis bundle are not required.

The workspace release profile sets `strip = "debuginfo"`. On this Mac that strip mis-aligns proc-macro dylibs, and dyld then fails the build with `can't find crate for zerofrom_derive`. The environment variable turns strip off for this binary only. `cargo build --release` for the Syrtis static library stays stripped. Linux can use the same variable. It does not change the app profile.

## Run

```sh
./target/release/tokenbar-quota
./target/release/tokenbar-quota --table
```

No flag prints pretty JSON. `--json` is the same. `--table` prints one text row per window.

A poll that reaches a provider also writes one pace sample, the same write the menu bar makes, into:

```text
~/Library/Application Support/com.nyanako.tokenbar/quota-pace-history-v3.json
```

The command does not read that file to invent a remaining percent. Antigravity can show a live percent here while its pace series still has no samples.

## What each row means

| Field | Meaning |
|---|---|
| `providerId` | `claude`, `codex`, `grok`, `antigravity`, `cursor`, `opencode`, `pi`, or `copilot` when Copilot is signed in |
| `account` | Email the provider already returned, when it has one |
| `accountKey` | Extra Claude config directory, when the card is not the primary account |
| `windowKey` | Provider window id, such as `five_hour.v1` or `plan.period.v1` |
| `remainingPercent`, `usedPercent` | Present only when `status` is `readable` |
| `resetAt` | ISO-8601 reset time, or null when the provider did not send one |
| `sampledAt` | When this poll read the window |
| `source` | `live` for this poll, `last-good` for a failed poll that still held a previous card |
| `providerSource` | `oauth`, `cli`, `dashboard`, or `none` |
| `status` | `readable` or `unreadable` |
| `reason` | Why a row is unreadable. Null on a readable row |

`last-good` rows stay `unreadable`. Their percents are omitted so a stale card is not reported as the live remaining.

OpenCode and Pi are always `unreadable`. TokenBar has no plan meter for them. An OpenCode OAuth label is a name, not a percent.

## Where the Mac credentials come from

The dump uses the readers the app already uses, plus the Cursor desktop token for the plan row.

| Provider | Location |
|---|---|
| Claude | Keychain item `Claude Code-credentials`, then `~/.claude/.credentials.json` |
| Codex | `$CODEX_HOME/auth.json`, or `~/.codex/auth.json` |
| Grok | `$GROK_HOME/auth.json`, or `~/.grok/auth.json` |
| Antigravity | Running IDE `language_server` (`GetUserStatus` on loopback), then `~/.gemini/oauth_creds.json` (`GEMINI_CLI_HOME` overrides the directory) |
| Cursor plan | `~/Library/Application Support/Cursor/User/globalStorage/state.vscdb` keys `cursorAuth/accessToken`, `cursorAuth/refreshToken`, and `cursorAuth/cachedEmail`. If the access token is not a JWT, Keychain item `cursor-access-token` |
| OpenCode labels | `$XDG_DATA_HOME/opencode/auth.json`, or `~/.local/share/opencode/auth.json` |

Cursor plan remaining comes from `POST https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage`. Reset time is `billingCycleEnd` on that response, or `planInfo.billingCycleEnd` from `GetPlanInfo` when the usage response has none. The command does not write a refreshed token back into Cursor's database.

## Check it on the Air

1. Sign in to Claude, Codex, Grok, and Cursor. Leave the Antigravity IDE running if you want the language-server reading.
2. Build `tokenbar-quota` with the command above.
3. Run `./target/release/tokenbar-quota > /tmp/quota.json`.
4. Confirm the file contains one object per provider you care about:

```sh
python3 -c 'import json; rows=json.load(open("/tmp/quota.json"))["windows"];
[print(r["providerId"], r["status"], r.get("remainingPercent"), r.get("resetAt"), r.get("reason")) for r in rows]'
```

5. For a `readable` Claude or Codex row, compare `remainingPercent` with the Syrtis quota card. They come from the same poll.
6. Antigravity should be `readable` with a remaining percent when the IDE or `~/.gemini` login works. If neither works, the row is `unreadable` and `reason` says why. It is not omitted.
7. Cursor should be `readable` with `windowKey` `plan.period.v1`, a remaining percent, and `resetAt`, or `unreadable` with a reason. A missing database and a missing keychain item are named. A non-JWT `cursorAuth/accessToken` is named as ciphertext. No percent is filled in for those cases.

Quit Syrtis first if you do not want this poll and the menu bar to write the pace file at the same time. The history store is locked, and both writers are the same one.
