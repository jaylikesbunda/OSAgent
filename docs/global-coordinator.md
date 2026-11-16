# Global chat

Use **Global chat** in the sidebar to open the same persistent conversation
across restarts and workspace changes. It is stored in the local session
database. Deleting the chat (or all chats) deletes its history; opening it
again creates a fresh conversation.

Global chat has no project workspace. For project work, specify which configured
workspace to use. The coordinator can delegate work to subagents, including in
the background; each child keeps its chosen workspace when resumed. If the
workspace is removed, that child cannot be resumed in a different workspace.
Existing workspace and session-access permissions still apply.

Global chat and regular chats can message each other, and any chat can
coordinate its subagents (parent, children and siblings), with the `agents`
tool. `send` shares information without starting a turn; `followup` hands over a
task and starts an idle recipient. Messages are labelled with their sender,
delivered at a boundary between model requests, kept across stops and
restarts, and subject to session-access permissions across conversations. See
[Agent communication](agent-communication.md).

The chat uses OSA's existing transcript archive, context compaction, working
notes and global memory facilities. Model context is finite: older conversation
content may be summarized, and persistent memory depends on its settings. Search
history or ask for details when exact recall matters. Global chat can use file,
shell, research and integration tools directly. Supply an absolute path for
file operations or an absolute working directory for shell commands; OSA applies
the configured external-path and sandbox permissions. Some workspace-dependent
bridges still require delegation to a workspace agent.
