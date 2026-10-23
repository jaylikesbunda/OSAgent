---
title: "GitHub PR Reviews"
description: "CodeRabbit-style pull request reviews: post inline comments from chat or let OSA review new commits automatically."
weight: 20
toc: true
---

## What this does

OSA reviews pull requests the way CodeRabbit does: it reads the diff,
finds real issues (bugs, security risks, unsafe edge cases), and posts a
review to GitHub with a summary plus inline comments — severity-tagged and
with `suggestion` blocks when the fix is obvious.

There are three ways to trigger it:

- **On demand** — ask in chat: *"review PR #123 in owner/repo"*. The
  `github` tool (found through tool search) fetches the diff, runs the
  review model, and posts the review.
- **On new commits** — watch repositories in **Settings → GitHub** with
  *On new commits* enabled; any open PR that gets new commits is reviewed
  and the review is posted. Drafts, ignored authors, and commits that
  were already reviewed are skipped, so nothing is reviewed twice.
- **On @mention** — with *On @mention* enabled, any conversation comment
  that tags the handle (`@osagent take a look`) triggers a review of that
  PR, with optional flags: `--dry-run`, `--approve`, `--request-changes`.
  A mention always re-reviews, even the same commit.

## Connect GitHub

Open **Settings → GitHub**. Two options:

1. **Sign in with GitHub** (zero setup). OSA ships a shared OAuth app for
   this; clicking the button opens GitHub in a new tab with the
   authorization code already filled in — click **Authorize** and the
   pane connects itself. No app registration, no code to type, no client
   secret on your side. If you prefer to use your own OAuth app instead
   (create one at github.com/settings/applications/new with **Enable
   Device Flow** checked), paste its Client ID into the pane's optional
   override.
2. **Personal access token** (works without any OAuth app, and the right
   choice for automation accounts). Create one at
   github.com/settings/tokens — classic with `repo` (plus `read:user`),
   or a fine-grained token with Pull requests (read/write), Issues
   (write), Contents (read), and Checks/Commit statuses (read/write).
   Paste it in and press *Save token*.

## Review settings

- **Watched repositories** — the repos the auto-reviewer polls (required
  for both triggers below).
- **On new commits** — review open PRs automatically when they get new
  commits.
- **On @mention** — review a PR when any comment on it contains
  `@<handle>` (default handle: `osagent-review`). Conversation comments **and
  inline review threads** are both scanned. OSA reacts with 👀 to the
  comment while it works and replies with the review link when it is done.
  Flags in the comment are honored: `--dry-run` runs the review locally
  without posting, `--approve` / `--request-changes` override the review
  event for that run. A mention forces a fresh review even if the current
  commit was already reviewed. Each comment triggers once; when the
  trigger is first enabled, comments older than a day are ignored so old
  history is not re-reviewed. The handle is matched as plain text, so it
  does not need to belong to a real GitHub account — pick something
  nobody owns (`@osagent-review` by default) to avoid notifying a
  stranger.
- **Poll interval** — how often OSA checks for new commits and mentions
  (30s–1h).
- **Review event** — `COMMENT` (default), `REQUEST_CHANGES`, or
  `APPROVE`. `COMMENT` posts findings without blocking the PR.
- **Max inline comments** — findings beyond the cap are listed in the
  review summary instead of inline.
- **Review model override** — `provider:model` (for example
  `anthropic:claude-sonnet-4-5`). Empty uses the current chat model.
- **Review instructions** — standing rules injected into every review
  ("we target Rust 1.78; never suggest rewriting tests").
- **Ignore authors / draft PRs** — keeps bot PRs and work-in-progress out
  of the commit-triggered loop (an explicit @mention still reviews).

## Manual runs and dry runs

- In chat: *"review PR 42 in acme/app"* — posts the review.
- From the pane's **Review a pull request now** card, uncheck *Post to
  GitHub* to see the review locally without publishing it.
- The chat tool also supports `review_pr` with `post: false` for dry
  runs, and custom reviews via `pr_review` (your own body + inline
  comments).

Every run is recorded under **Review history**, with a link to the posted
review, and commits already reviewed are marked so the auto-reviewer
skips them.

## Notes

- Reviews run on the diff plus PR metadata (title, description, commit
  messages). Very large diffs are reviewed in several passes; files the
  API reports without a patch (binaries, huge files) can only receive
  general comments.
- The engine validates every inline comment against the diff hunks before
  posting. Findings that cannot be pinned to a line are listed in the
  review body, so a review never fails because of a bad line number.
- If posting inline comments is rejected (stale diff, forked repo), OSA
  falls back to a summary-only review, then to a plain PR comment.

## Related tasks

- [Scheduled tasks]({{< relref "scheduled-tasks.md" >}})
- [Tool overview]({{< relref "../tools/overview.md" >}})
