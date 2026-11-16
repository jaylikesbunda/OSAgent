# Agent communication

Chats, Global chat and their subagents can send each other structured
messages with the `agents` tool. The design follows the multi-agent model in
Codex ([agent control](https://github.com/openai/codex/blob/main/codex-rs/core/src/agent/control.rs),
[tool definitions](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/multi_agents_spec.rs)),
adapted to OSA's durable sessions.

## Actions

| Action | Effect |
| --- | --- |
| `list` | Shows the agents in your conversation tree, how they relate to you, and their state (running, idle, completed, failed, stopped). |
| `send` | Delivers information. **Never starts a turn.** A running agent sees the message before its next model request; an idle agent sees it at the start of its next turn. |
| `followup` | Gives an agent a new task. A running agent sees it before its next model request. An idle chat starts a turn. An idle subagent resumes in the background with its history and workspace. |
| `start_chat` | Global chat only. Opens a new regular chat in a configured workspace (`workspace_id`, default: the active workspace) with an optional `title`, and gives it `message` as a follow-up task. The chat appears in the sidebar, starts working, and its answer comes back to Global chat as a reply. |

A subagent's final answer is delivered to its parent automatically, as it was
before. A child should not also `send` it.

The same holds between chats. When a chat finishes a turn in which it handled a
`followup` (or `start_chat` task) from another chat, its answer goes back to
that chat as a **reply**. A reply starts an idle recipient's turn, so Global
chat reports a started chat's result without being asked. A reply is never
answered automatically, so two chats cannot bounce answers back and forth.
Past the hand-off limit the reply is still delivered, but without starting a
turn.

Each subagent gets a first name that is unique among its siblings, plus its job
from the task description, such as “Dylan · Repo inspector”. That label is
used in messages, `list` and the sidebar.

`target` is an agent's session ID from `list`, a unique ID prefix of at least
six characters for agents in your tree, `parent`, `root` or `global`.

## Who can message whom

- **Within a conversation tree** (a chat and all its subagents): any agent can
  message any other, including parent, children and siblings. These messages
  need no approval because the tree is already one task.
- **Follow-ups go down or sideways.** An agent cannot hand a follow-up task to
  its parent or another ancestor. It reports up with `send` or its final
  answer, as in Codex.
- **Across conversations**: only the chats themselves can communicate, and only
  between a regular chat and Global chat. Session-access permissions (rules,
  default action and approval prompts) apply to each message. Subagents ask
  their parent to contact another chat.
- Discord community sessions cannot take part.

## Delivery

Messages are stored in the local database before the tool returns. Only the
recipient's own run adds them to its conversation, at a boundary between model
requests (after the pending tool calls finish), so a model request or tool call
is never interrupted. The message is recorded in the conversation before it is
marked delivered; a crash can repeat a message but does not lose it. If the
same tool call is retried, the duplicate is ignored.

The recipient sees:

```text
<agent_message type="FOLLOWUP_TASK" from="Explore agent" from_id="…" relation="sibling">
…
</agent_message>
```

The model is told that this is another agent's output, not a user instruction.
In the web UI it appears as “Follow-up from Explore agent (sibling)”, not
“You”.

| Recipient state | `send` | `followup` |
| --- | --- | --- |
| Running | Before its next model request | Before its next model request. If a chat finishes first, a new turn starts once that run ends. A subagent that was about to finish handles the follow-up before returning its answer. |
| Idle chat | At its next turn | Starts a turn, using the same mechanism as background-result wakes |
| Idle subagent | At its next run | Resumes the subagent in the background, in its original workspace. A Global chat child whose workspace was removed is not moved elsewhere; the message stays queued and the tool reports the error. |
| Stopped chat (or a subagent of one) | Kept | Kept; no turn starts until the user continues the chat |
| After an OSA restart | Kept | Kept; delivered at the recipient's next turn. OSA does not start turns on its own at startup. |

## Preventing loops

`send` cannot start a turn, so it cannot cause a loop. For `followup`, OSA
tracks a hand-off chain for each conversation:

- A turn started by a person resets the chain to 0.
- A follow-up carries the sender's chain plus one. A turn or subagent run
  started by a follow-up inherits that value. New subagents inherit their
  parent's value.
- Follow-ups that would exceed `subagent_auto_resume_max_turns` (default 5)
  are refused, and the agent is told to finish with what it has.

Agents cannot wait for a reply, so a model missing an input tends to poll by
re-sending. OSA refuses a message that repeats one of the sender's recent
messages to the same recipient (ignoring case and spacing), and refuses a third
unread message from one sender to one recipient. The refusal tells the agent to
continue with what it has or finish and say what is missing.

The existing limit on consecutive automatic turns per chat also applies to chat
wake-ups. One recipient can hold at most 50 unread messages. Messages are
limited to 16,000 characters.

This replaces the earlier `sessions(action="send")` relay and its one-reply
limit. The `sessions` tool is read-only again.

## Differences from Codex

- Codex keeps its agent tree in memory for one root session. OSA stores
  messages and the session tree in SQLite, so messages survive restarts.
- Codex addresses agents by task paths such as `/root/task1`. OSA uses session
  IDs and the aliases `parent`, `root` and `global`.
- Codex does not wake the parent for child completion messages; the parent
  waits with `wait_agent`. OSA continues to merge background results into the
  parent and wake it (subject to `subagent_auto_resume`), so there is no
  `wait` tool.
- Cross-conversation messaging between Global chat and regular chats is
  specific to OSA.
