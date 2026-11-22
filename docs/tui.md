# Terminal UI

Run `cargo run --bin axon` from the repo's root folder to start the terminal UI. Type a prompt and press Enter to send it.

## Keys

| Key | Action |
| --- | --- |
| Enter | Send the prompt. During a turn, queue it |
| Ctrl+Enter, Ctrl+S | During a turn, stop it and send the queue now |
| Shift+Enter, Ctrl+J | New line |
| Arrow keys | Move the cursor |
| ↑ on the first row, ↓ on the last | Previous or next sent prompt |
| Alt+←/→, Ctrl+←/→, Alt+B/F | Move by word |
| Home/End, Ctrl+A/E | Start or end of the line |
| PageUp/PageDown, mouse wheel | Scroll the history |
| Ctrl+End | Jump to the bottom of the history |
| Y / N | Approve or reject a change. While a change waits, other keys only cancel, and typing is ignored |
| Esc, Ctrl+C | Cancel the turn |
| Ctrl+C when idle | Quit |

## Commands

| Command | Action |
| --- | --- |
| `/exit` (alias `/quit`) | Quit, even during a turn |

## Queued prompts

Prompts sent during a turn wait below it as `Next`. When the turn ends, whether it finishes, fails or is cancelled, they are sent together as one message. Ctrl+Enter adds the input to the queue and stops the turn at once, keeping the reply so far.

Ctrl+Enter needs a terminal with the kitty keyboard protocol, such as kitty, WezTerm, Ghostty, iTerm2, Alacritty or foot. On Windows it works in any console. Other terminals, including macOS Terminal and tmux by default, send it as Enter (it queues) or as Ctrl+J (a new line). Ctrl+S works everywhere.

## Pastes

A paste longer than 1000 characters shows as a single `[pasted N chars]` token in the input and the history. The model gets the full text. Set `AXON_PASTE_TOKEN_CHARS` to change the limit.

## Context size

At the start of a session Axon asks the server for the model's context size and shows it after each turn, for example `✓ Finished at 14:32 · 2m 5s · 262k context`. Every `AXON_BACKEND` except `openai` reports it. Ollama and LM Studio report it only while the model is loaded.

## Sessions

Axon saves the session after each turn, in `~/.axon/sessions` (`%USERPROFILE%\.axon\sessions` on Windows), or in `AXON_SESSIONS_DIR` if set. When you quit, it prints the command to resume.

| Command | Resumes |
| --- | --- |
| `axon --continue` | The last session used in the current folder |
| `axon --resume <id>` | The session in the current folder whose id starts with `<id>` |

A resumed session shows its history, and the model gets the conversation so far.
