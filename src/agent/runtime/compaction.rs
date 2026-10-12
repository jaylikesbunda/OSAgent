//! Message classification + context compaction + working notes
//! Extracted verbatim from runtime.rs; behavior-preserving split.
use super::*;

impl AgentRuntime {
    pub(super) fn session_metadata_mut(
        session: &mut Session,
    ) -> Result<&mut serde_json::Map<String, serde_json::Value>> {
        if !session.metadata.is_object() {
            session.metadata = serde_json::json!({});
        }

        session.metadata.as_object_mut().ok_or_else(|| {
            OSAgentError::Parse("Session metadata must be a JSON object".to_string())
        })
    }

    pub(super) fn push_session_event(
        session: &mut Session,
        event_type: &str,
        data: serde_json::Value,
    ) -> Result<()> {
        let metadata = Self::session_metadata_mut(session)?;
        let events = metadata
            .entry("session_events".to_string())
            .or_insert_with(|| serde_json::json!([]));
        let array = events.as_array_mut().ok_or_else(|| {
            OSAgentError::Parse("Session events metadata must be an array".to_string())
        })?;

        array.push(
            serde_json::to_value(SessionEventRecord {
                event_type: event_type.to_string(),
                timestamp: Utc::now(),
                data,
            })
            .map_err(|e| OSAgentError::Parse(format!("Failed to encode session event: {}", e)))?,
        );

        if array.len() > 200 {
            let drop_count = array.len() - 200;
            array.drain(0..drop_count);
        }

        Ok(())
    }

    pub(super) fn message_kind(message: &Message) -> Option<&str> {
        message
            .metadata
            .get("kind")
            .and_then(|value| value.as_str())
    }

    pub(super) fn is_synthetic_message(message: &Message) -> bool {
        message
            .metadata
            .get("synthetic")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
    }

    pub(super) fn is_real_user_message(message: &Message) -> bool {
        message.role == "user" && !Self::is_synthetic_message(message)
    }

    /// Whether a stored compaction marker is superseded by a newer one.
    /// Only the newest summary is forwarded to the provider; older ones
    /// stay in storage and the archive but are never re-sent (and
    /// re-summarized) on later turns.
    pub(super) fn is_superseded_summary(messages: &[Message], index: usize) -> bool {
        let message = &messages[index];
        if !Self::is_synthetic_message(message)
            || Self::message_kind(message) != Some("compaction_summary")
        {
            return false;
        }
        messages[index + 1..].iter().any(|later| {
            Self::is_synthetic_message(later)
                && Self::message_kind(later) == Some("compaction_summary")
        })
    }

    pub(super) fn looks_like_internal_tool_dump(content: &str) -> bool {
        let trimmed = content.trim();
        let lower = trimmed.to_lowercase();

        trimmed.starts_with("OLCALL>")
            || lower.starts_with("tool_calls")
            || (trimmed.starts_with('[')
                && lower.contains("\"name\"")
                && lower.contains("\"arguments\""))
            || (trimmed.starts_with('{')
                && lower.contains("\"name\"")
                && lower.contains("\"arguments\""))
    }

    pub(super) fn is_visible_assistant_message(message: &Message) -> bool {
        message.role == "assistant"
            && !message.content.trim().is_empty()
            && message
                .tool_calls
                .as_ref()
                .map(|calls| calls.is_empty())
                .unwrap_or(true)
            && !Self::looks_like_internal_tool_dump(&message.content)
    }

    /// Remove model-emitted `<tool_call>...</tool_call>` text blocks (and a
    /// dangling unclosed opener) from text fed to the summarizer. The client
    /// never parses this Qwen/GLM-style convention into structured calls, so
    /// leaving it in pollutes summaries with raw call markup.
    pub(super) fn strip_tool_call_markup_text(text: &str) -> String {
        let mut out = text.to_string();
        loop {
            let lower = out.to_lowercase();
            let Some(start) = lower.find("<tool_call>") else {
                break;
            };
            if let Some(end_rel) = lower[start..].find("</tool_call>") {
                let end = start + end_rel + "</tool_call>".len();
                out.replace_range(start..end, "");
            } else {
                out.truncate(start);
                break;
            }
        }
        out
    }

    /// Unwrap a previously framed `<compacted-summary>...</compacted-summary>`
    /// body to its inner text so re-compaction merges summaries instead of
    /// nesting framing tags inside each other.
    pub(super) fn unwrap_compacted_summary_text(text: &str) -> &str {
        let lower = text.to_lowercase();
        if let Some(start_rel) = lower.find("<compacted-summary>") {
            let inner_start = start_rel + "<compacted-summary>".len();
            if let Some(end_rel) = lower[inner_start..].find("</compacted-summary>") {
                return text[inner_start..inner_start + end_rel].trim();
            }
        }
        text
    }

    pub(super) fn compactable_message_content(message: &Message) -> Option<String> {
        if message.role == "system" {
            return None;
        }
        // Compaction summaries merge into the next summary like opencode's
        // prior summaries; every other synthetic control message is skipped.
        if Self::is_synthetic_message(message)
            && Self::message_kind(message) != Some("compaction_summary")
        {
            return None;
        }

        let mut content = message.content.replace('\n', " ").trim().to_string();
        // Prior summaries feed back in unwrapped so tags never nest, and
        // unparsed tool-call markup is dropped before it can leak into the
        // next summary.
        if Self::message_kind(message) == Some("compaction_summary") {
            content = Self::unwrap_compacted_summary_text(&content).to_string();
        }
        content = Self::strip_tool_call_markup_text(&content)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if content.is_empty() {
            // Tool-call-only assistant turns and image-only user turns still
            // carry signal; describe the shape instead of dropping them.
            if let Some(calls) = message
                .tool_calls
                .as_ref()
                .filter(|calls| !calls.is_empty())
            {
                let names: Vec<String> = calls.iter().map(|call| call.name.clone()).collect();
                content = format!("[tool calls: {}]", names.join(", "));
            } else if !message.images.is_empty() {
                content = format!("[{} attached image(s)]", message.images.len());
            }
        }
        if let Some(thinking) = message
            .thinking
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            let short: String = thinking.replace('\n', " ").chars().take(300).collect();
            content = format!("{} [reasoning: {}]", content, short.trim());
        }
        if content.is_empty() {
            return None;
        }

        // Keep enough exact local evidence for checkpoint generation. The
        // transcript-level token budget remains the hard bound; this per-item
        // guard only prevents one giant result from monopolizing it.
        let per_message_cap = if message.role == "tool" {
            4_000
        } else {
            12_000
        };
        if content.chars().count() > per_message_cap {
            content = content.chars().take(per_message_cap).collect::<String>();
            content.push_str("... [truncated]");
        }

        let label = match (message.role.as_str(), Self::message_kind(message)) {
            ("assistant", Some("compaction_summary")) => "assistant_summary",
            (role, Some(kind)) => return Some(format!("{}({}): {}", role, kind, content)),
            (role, None) => role,
        };

        Some(format!("{}: {}", label, content))
    }

    /// Budgeted transcript for the summarizer. Foundational anchors are
    /// always retained (first request, prior checkpoint, latest requests),
    /// then remaining room is filled newest-first. This avoids the former
    /// fixed 24KB tail-only view silently dropping the original objective.
    pub(super) fn transcript_for_compaction(messages: &[Message], max_chars: usize) -> String {
        let lines: Vec<Option<String>> = messages
            .iter()
            .map(Self::compactable_message_content)
            .collect();
        let mut mandatory = std::collections::BTreeSet::new();
        if let Some(index) = messages.iter().position(Self::is_real_user_message) {
            mandatory.insert(index);
        }
        if let Some(index) = messages
            .iter()
            .rposition(|message| Self::message_kind(message) == Some("compaction_summary"))
        {
            mandatory.insert(index);
        }
        for (index, _) in messages
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, message)| Self::is_real_user_message(message))
            .take(2)
        {
            mandatory.insert(index);
        }

        let mut selected = std::collections::BTreeSet::new();
        let mut used = 0usize;
        for index in mandatory {
            let Some(line) = lines.get(index).and_then(Option::as_ref) else {
                continue;
            };
            let remaining = max_chars.saturating_sub(used);
            if remaining == 0 {
                break;
            }
            let take = line.chars().count().min(remaining);
            used += take;
            selected.insert(index);
        }
        for index in (0..lines.len()).rev() {
            if selected.contains(&index) {
                continue;
            }
            let Some(line) = lines[index].as_ref() else {
                continue;
            };
            let extra = usize::from(!selected.is_empty());
            if used + line.chars().count() + extra > max_chars {
                continue;
            }
            used += line.chars().count() + extra;
            selected.insert(index);
        }

        if selected.is_empty() {
            "No earlier transcript available.".to_string()
        } else {
            let omitted = lines.iter().filter(|line| line.is_some()).count() - selected.len();
            let mut rendered = Vec::new();
            for index in selected {
                if let Some(line) = lines[index].as_ref() {
                    let remaining = max_chars.saturating_sub(
                        rendered
                            .iter()
                            .map(|line: &String| line.chars().count() + 1)
                            .sum(),
                    );
                    rendered.push(line.chars().take(remaining).collect());
                }
            }
            if omitted > 0 {
                rendered.push(format!(
                    "[{} lower-priority message(s) omitted by checkpoint budget]",
                    omitted
                ));
            }
            rendered.join("\n")
        }
    }

    /// Facts that can be recovered exactly from canonical messages should not
    /// depend on the summarizer remembering them. They are supplied beside the
    /// prose transcript and carried into the structured checkpoint.
    pub(super) fn deterministic_compaction_facts(messages: &[Message]) -> String {
        let mut read_files = std::collections::BTreeSet::new();
        let mut modified_files = std::collections::BTreeSet::new();
        for message in messages {
            let Some(calls) = message.tool_calls.as_ref() else {
                continue;
            };
            for call in calls {
                let target = match call.name.as_str() {
                    "write_file" | "edit_file" | "delete_file" | "apply_patch" => {
                        &mut modified_files
                    }
                    "read_file" | "grep" | "glob" | "codesearch" => &mut read_files,
                    _ => continue,
                };
                if let Some(path) = call.arguments.get("path").and_then(|v| v.as_str()) {
                    if !path.trim().is_empty() {
                        target.insert(path.trim().to_string());
                    }
                }
                if let Some(paths) = call.arguments.get("paths").and_then(|v| v.as_array()) {
                    for path in paths.iter().filter_map(|v| v.as_str()) {
                        if !path.trim().is_empty() {
                            target.insert(path.trim().to_string());
                        }
                    }
                }
                if call.name == "apply_patch" {
                    if let Some(patch) = call.arguments.get("patch").and_then(|v| v.as_str()) {
                        modified_files.extend(Self::patch_paths_for_snapshot(patch));
                    }
                }
            }
        }
        let latest_requests: Vec<String> = messages
            .iter()
            .rev()
            .filter(|message| Self::is_real_user_message(message))
            .take(2)
            .map(|message| message.content.trim().chars().take(4_000).collect())
            .collect::<Vec<String>>()
            .into_iter()
            .rev()
            .collect();
        format!(
            "Latest genuine user requests:\n{}\n\nFiles read/searched:\n{}\n\nFiles modified/requested for modification:\n{}",
            if latest_requests.is_empty() { "(none)".to_string() } else { latest_requests.join("\n---\n") },
            if read_files.is_empty() { "(none)".to_string() } else { read_files.into_iter().collect::<Vec<_>>().join("\n") },
            if modified_files.is_empty() { "(none)".to_string() } else { modified_files.into_iter().collect::<Vec<_>>().join("\n") },
        )
    }

    /// Model-free pre-compaction pass: rewrite over-budget tool results
    /// to a bounded head/tail slice with a fixed middle marker. No LLM
    /// call; a second pass over already-pruned content is provably a
    /// no-op because `head + marker + tail <= threshold` is clamped.
    /// Ported from DSH's `compaction-tool-result-pruner`.
    pub(super) fn prune_old_tool_messages(
        &self,
        session: &mut Session,
        preserve_from: usize,
        compaction: &crate::config::CompactionConfig,
    ) -> usize {
        const MIDDLE_MARKER: &str = "\n[... tool result middle pruned ...]\n";

        let mut pruned = 0usize;
        let threshold = compaction.prune_threshold_chars;
        let head = compaction.prune_head_chars;
        let tail = compaction
            .prune_tail_chars
            .min(threshold.saturating_sub(head + MIDDLE_MARKER.len()));

        let call_names: std::collections::HashMap<String, String> = session
            .messages
            .iter()
            .filter_map(|message| message.tool_calls.as_ref())
            .flatten()
            .map(|call| (call.id.clone(), call.name.clone()))
            .collect();

        // `preserve_from` is the retained-tail split: everything before it is
        // about to be archived byte-identical, so pruning must only touch the
        // retained tail. This keeps the archive faithful and leaves the
        // summarizer the full original prefix.
        for message in session.messages.iter_mut().skip(preserve_from) {
            if message.role != "tool" {
                continue;
            }
            if message
                .metadata
                .get("pruned_for_context")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                continue;
            }
            let chars = message.content.chars().count();
            if chars <= threshold {
                continue;
            }

            let head_text: String = message.content.chars().take(head).collect();
            let tail_text: String = message
                .content
                .chars()
                .rev()
                .take(tail)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            // Preserve the exact result as a retrievable artifact before
            // rewriting active context. Existing spill references are reused.
            let existing_spill = message
                .metadata
                .get("spill_path")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            let tool_name = message
                .tool_call_id
                .as_ref()
                .and_then(|id| call_names.get(id))
                .cloned()
                .unwrap_or_else(|| "tool-result".to_string());
            let spill_path = existing_spill.or_else(|| {
                self.spill_store
                    .save(
                        &session.id,
                        &format!("compaction-{}", tool_name),
                        &message.content,
                    )
                    .ok()
                    .map(|spill| spill.path.to_string_lossy().to_string())
            });
            let recovery = spill_path
                .as_ref()
                .map(|path| format!("\n[Full original tool result archived at: {}]", path))
                .unwrap_or_default();
            message.content = format!("{}{}{}{}", head_text, MIDDLE_MARKER, tail_text, recovery);
            if let (Some(path), Some(metadata)) = (spill_path, message.metadata.as_object_mut()) {
                metadata.insert("spilled".to_string(), serde_json::json!(true));
                metadata.insert("spill_path".to_string(), serde_json::json!(path));
                metadata.insert("spill_bytes".to_string(), serde_json::json!(chars));
            }
            message.metadata["pruned_for_context"] = serde_json::json!(true);
            pruned += 1;
        }

        pruned
    }

    /// Persist a provider-side context compression into the session: the middle
    /// of the history is replaced with the summary the provider produced, so the
    /// model sees it on later turns instead of the compression being lost after
    /// the single request.
    ///
    /// Returns the removed span so the caller can archive it before
    /// persisting; `None` when there was nothing to compact.
    /// Plan a provider-side compression splice without mutating the session,
    /// so the caller can archive the dropped span first and only splice on
    /// success (fail closed: history is never replaced unless archived).
    /// Returns `(keep_head, keep_tail, dropped, framed_summary)`.
    /// The summary uses the standard `<compacted-summary>` framing so later
    /// local compactions merge it instead of nesting foreign formats.
    pub(super) fn plan_provider_compaction(
        session: &Session,
        summary: &str,
    ) -> Option<(usize, usize, Vec<Message>, String)> {
        let original_len = session.messages.len();
        if original_len < 8 {
            return None;
        }
        let keep_tail = 6usize.min(original_len.saturating_sub(4));
        let keep_head = 3usize.min(original_len.saturating_sub(keep_tail));
        if original_len <= keep_head + keep_tail + 1 {
            return None;
        }
        let dropped = session.messages[keep_head..original_len - keep_tail].to_vec();
        // Fail closed if the cut would orphan a tool call/result pair: the
        // provider rejects histories whose results lost their calls (or
        // vice versa), so decline the splice instead of persisting one.
        let cut = original_len - keep_tail;
        let head_call_dropped_result = session.messages[..cut].iter().any(|message| {
            message.tool_calls.as_ref().is_some_and(|calls| {
                calls.iter().any(|call| {
                    session.messages[cut..]
                        .iter()
                        .any(|dropped| dropped.tool_call_id.as_deref() == Some(call.id.as_str()))
                })
            })
        });
        let tail_starts_orphaned_result = session
            .messages
            .get(cut)
            .filter(|first| first.role == "tool")
            .is_some_and(|first| {
                !session.messages[..cut].iter().any(|kept| {
                    kept.tool_calls.as_ref().is_some_and(|calls| {
                        calls
                            .iter()
                            .any(|call| first.tool_call_id.as_deref() == Some(call.id.as_str()))
                    })
                })
            });
        if head_call_dropped_result || tail_starts_orphaned_result {
            return None;
        }
        let framed = Self::frame_compacted_summary(summary);
        Some((keep_head, keep_tail, dropped, framed))
    }

    /// Frame a compaction summary the way DSH does: a single
    /// `<compacted-summary>` block plus a preamble telling the model not
    /// to acknowledge it, so the replacement reads as a continuation
    /// checkpoint rather than a fresh message to respond to.
    pub(super) fn frame_compacted_summary(summary: &str) -> String {
        format!(
            "<compacted-summary>\n{}\n</compacted-summary>\n\nDo not acknowledge the compacted summary explicitly in your reply.",
            summary.trim()
        )
    }

    pub(super) fn valid_checkpoint_summary(summary: &str, max_tokens: usize) -> bool {
        const HEADINGS: [&str; 6] = [
            "## Primary Request and Intent",
            "## Key Technical Concepts",
            "## Errors and Fixes",
            "## Files and Code Sections",
            "## Problem Solving",
            "## Next Step",
        ];
        let trimmed = summary.trim();
        !trimmed.is_empty()
            && Self::estimate_tokens(trimmed) <= max_tokens.max(512)
            && HEADINGS.iter().all(|heading| trimmed.contains(heading))
    }

    pub(super) fn deterministic_checkpoint(messages: &[Message], token_budget: usize) -> String {
        let facts = Self::deterministic_compaction_facts(messages);
        let recent = Self::summarize_for_context(messages, token_budget.saturating_sub(500));
        format!(
            "## Primary Request and Intent\n- See exact latest requests in deterministic context below.\n\n## Key Technical Concepts\n- (deterministic fallback; inspect retained recent context)\n\n## Errors and Fixes\n- (none captured deterministically)\n\n## Files and Code Sections\n{}\n\n## Problem Solving\n{}\n\n## Next Step\n- Continue from the latest retained user request.\n\n### Deterministic Context\n{}",
            facts,
            recent,
            facts,
        )
    }

    /// Compact-and-retry after a provider context-length rejection.
    /// Returns `Ok(Some(true))` when history shrank and the caller should
    /// rebuild the request (`continue`), `Ok(Some(false))` when compaction
    /// ran but nothing shrank, and `Ok(None)` when there was nothing to
    /// compact at all.
    pub(super) async fn compact_after_context_error(
        &self,
        session_id: &str,
        session: &mut Session,
        active_workspace: &WorkspaceConfig,
        runtime_config: &Config,
        iteration: usize,
        error: &OSAgentError,
    ) -> Result<Option<bool>> {
        let before: usize = session.messages.iter().map(Self::message_tokens).sum();
        match self
            .compact_session_history(
                session,
                active_workspace,
                &runtime_config.compaction,
                None,
                None,
            )
            .await?
        {
            Some((pruned, compacted, replayed)) => {
                if let Some(ref mut cs) = session.context_state {
                    cs.compaction_stats.total_compactions += 1;
                    cs.compaction_stats.total_pruned_messages += pruned;
                    cs.compaction_stats.total_compacted_messages += compacted;
                }
                self.record_session_event(
                    session,
                    "compaction",
                    serde_json::json!({
                        "iteration": iteration,
                        "pruned_messages": pruned,
                        "compacted_messages": compacted,
                        "replayed": replayed,
                        "source": "context_limit_retry",
                        "error": error.to_string(),
                    }),
                )?;
                self.event_bus.emit(AgentEvent::Compaction {
                    session_id: session_id.to_string(),
                    sequence: 0,
                    pruned_messages: pruned,
                    compacted_messages: compacted,
                    replayed,
                    timestamp: SystemTime::now(),
                });
                self.session_manager.update_session(session).await?;
                let after: usize = session.messages.iter().map(Self::message_tokens).sum();
                if after < before {
                    self.emit_reasoning_event(
                        session_id,
                        format!(
                            "Provider rejected the request as over context ({} tokens); compacted to {} and retrying.",
                            before, after
                        ),
                    );
                    Ok(Some(true))
                } else {
                    Ok(Some(false))
                }
            }
            None => Ok(None),
        }
    }

    /// Split the transcript into a compactable prefix and a retained tail.
    /// The tail walks back whole user turns until it fits the configured
    /// recent-token budget, then advances past any dangling tool-result or
    /// tool-call pairing so the cut never orphans one side of the pair.
    /// When nothing fits (a single turn already exceeds the budget), the
    /// oldest turn boundary still compacts so pressure always shrinks.
    pub(super) fn compaction_split_index(
        messages: &[Message],
        compaction: &crate::config::CompactionConfig,
    ) -> usize {
        let tail_budget = compaction.preserve_recent_tokens.max(2_000);
        let turns: Vec<usize> = messages
            .iter()
            .enumerate()
            .filter(|(_, message)| Self::is_real_user_message(message))
            .map(|(index, _)| index)
            .collect();
        if turns.is_empty() {
            // No user turns (system/synthetic/tool-only history): still
            // compact by keeping the last few messages as the tail.
            let fallback = messages.len().saturating_sub(6).max(1).min(messages.len());
            let snapped = Self::snap_split_to_tool_boundary(messages, fallback).min(messages.len());
            return if snapped == 0 || snapped >= messages.len() {
                fallback.min(messages.len().saturating_sub(1).max(1))
            } else {
                snapped
            };
        }
        let mut split = *turns.last().unwrap_or(&0);
        let mut tail_tokens = 0usize;
        for position in (0..turns.len()).rev() {
            let turn_start = turns[position];
            let turn_end = turns.get(position + 1).copied().unwrap_or(messages.len());
            let turn_tokens: usize = messages[turn_start..turn_end]
                .iter()
                .map(Self::message_tokens)
                .sum();
            if tail_tokens + turn_tokens <= tail_budget {
                tail_tokens += turn_tokens;
                split = turn_start;
                continue;
            }
            break;
        }
        // When even the newest turn exceeds the budget, keep that newest
        // turn as the tail rather than giving up: compacting everything
        // except the latest turn still relieves pressure.
        if split == 0 && turns.len() > 1 {
            split = turns[turns.len() - 1];
        }
        // Never cut between an assistant tool call and its tool results, or
        // between tool results and the assistant turn that consumed them.
        // Snapping returns `messages.len()` when the whole remaining suffix
        // is one fused tool-pair block: the entire suffix then forms the
        // tail and the split is the earlier turn start we snapped from.
        let snapped = Self::snap_split_to_tool_boundary(messages, split);
        split = if snapped >= messages.len() {
            split
        } else {
            snapped
        };
        split = split.min(messages.len());
        // Single-turn sessions still need compaction when they are large:
        // fall back to keeping the last few messages as the tail.
        if split == 0 && messages.len() > 1 {
            let fallback = messages.len().saturating_sub(6).max(1);
            split = Self::snap_split_to_tool_boundary(messages, fallback).min(messages.len());
            if split == 0 || split >= messages.len() {
                split = fallback.min(messages.len().saturating_sub(1).max(1));
            }
        }
        split
    }

    /// Move a candidate split forward past tool-pairing boundaries: a split
    /// that would strand an assistant tool call without its tool results,
    /// or tool results without the assistant call that owns them, advances
    /// until both sides sit on the same side of the cut.
    pub(super) fn snap_split_to_tool_boundary(messages: &[Message], split: usize) -> usize {
        let mut adjusted = split.min(messages.len());
        while adjusted > 0 && adjusted < messages.len() {
            let in_prefix_call_ids: std::collections::HashSet<&str> = messages[..adjusted]
                .iter()
                .filter_map(|message| message.tool_calls.as_ref())
                .flat_map(|calls| calls.iter().map(|call| call.id.as_str()))
                .collect();
            let in_prefix_result_ids: std::collections::HashSet<&str> = messages[..adjusted]
                .iter()
                .filter_map(|message| message.tool_call_id.as_deref())
                .collect();
            let tail_result_ids: std::collections::HashSet<&str> = messages[adjusted..]
                .iter()
                .filter_map(|message| message.tool_call_id.as_deref())
                .collect();
            let tail_call_ids: std::collections::HashSet<&str> = messages[adjusted..]
                .iter()
                .filter_map(|message| message.tool_calls.as_ref())
                .flat_map(|calls| calls.iter().map(|call| call.id.as_str()))
                .collect();
            let stranded_call = in_prefix_call_ids
                .iter()
                .any(|id| !in_prefix_result_ids.contains(id) && tail_result_ids.contains(id));
            let stranded_result = in_prefix_result_ids
                .iter()
                .any(|id| !in_prefix_call_ids.contains(id) && tail_call_ids.contains(id));
            // A tail that opens mid-pair must pull the pair's other half
            // into the tail too: a tool result whose call is not in the
            // tail, or an assistant call whose result never follows in the
            // tail. A call whose result follows inside the tail is the
            // healthy case and must NOT advance.
            let tail_opens_orphaned = messages[adjusted..].first().is_some_and(|first| {
                let opens_with_result = first.role == "tool"
                    && first
                        .tool_call_id
                        .as_deref()
                        .is_some_and(|id| !tail_call_ids.contains(id));
                let opens_with_call = first
                    .tool_calls
                    .as_ref()
                    .is_some_and(|calls| !calls.is_empty())
                    && first.tool_calls.as_ref().is_some_and(|calls| {
                        !calls
                            .iter()
                            .any(|call| tail_result_ids.contains(call.id.as_str()))
                    });
                opens_with_result || opens_with_call
            });
            if stranded_call || stranded_result || tail_opens_orphaned {
                adjusted += 1;
                continue;
            }
            break;
        }
        adjusted
    }

    pub(super) async fn compact_session_history(
        &self,
        session: &mut Session,
        _active_workspace: &WorkspaceConfig,
        compaction: &crate::config::CompactionConfig,
        focus: Option<&str>,
        conversation_budget: Option<usize>,
    ) -> Result<Option<(usize, usize, bool)>> {
        let compact_end = Self::compaction_split_index(&session.messages, compaction);
        if compact_end == 0 || compact_end >= session.messages.len() {
            return Ok(None);
        }

        // The summarization pass that follows can take a while, so tell the
        // UI to show a persistent "Compacting" indicator for its duration.
        // The matching `Compaction` event on completion (or the next stream
        // event) clears it.
        self.event_bus.emit(AgentEvent::CompactionStarted {
            session_id: session.id.clone(),
            sequence: 0,
            timestamp: SystemTime::now(),
        });

        // Prune only the retained tail: rewriting the prefix before it is
        // archived would persist pruned text instead of the original content.
        let tail_start_for_prune = compact_end;
        let pruned = if compaction.prune_enabled {
            self.prune_old_tool_messages(session, tail_start_for_prune, compaction)
        } else {
            0
        };
        // Tool-result externalization may be sufficient by itself. Avoid an
        // unnecessary lossy model summary when the remeasured conversation is
        // now below the exact preflight conversation allowance.
        if pruned > 0
            && conversation_budget.is_some_and(|budget| {
                session
                    .messages
                    .iter()
                    .map(Self::message_tokens)
                    .sum::<usize>()
                    <= budget
            })
        {
            self.session_manager.update_session(session).await?;
            return Ok(Some((pruned, 0, false)));
        }
        // Re-split after pruning, since pruning never changes message count
        // but the tail token cost may have dropped; keep the same split.
        let prefix = session.messages[..compact_end].to_vec();
        let tail = session.messages[compact_end..].to_vec();
        if prefix.is_empty() {
            return Ok(if pruned > 0 {
                Some((pruned, 0, false))
            } else {
                None
            });
        }
        // Refuse to summarize trivia: when the prefix costs almost nothing,
        // a "summary" can only echo it back (or pad it), which reads as trash
        // and pollutes the next merge. Prune-only progress above is still
        // returned; otherwise the caller reports nothing to compact.
        // Floor is ~4KB of text: tool-output-heavy short sessions still pass
        // on token weight, not message count.
        const MIN_PREFIX_TOKENS: usize = 1_000;
        let prefix_tokens: usize = prefix.iter().map(Self::message_tokens).sum();
        if prefix_tokens < MIN_PREFIX_TOKENS {
            return Ok(if pruned > 0 {
                Some((pruned, 0, false))
            } else {
                None
            });
        }

        let existing_notes = session
            .context_state
            .as_ref()
            .and_then(|state| state.working_notes.as_ref())
            .map(|notes| notes.text.trim().to_string())
            .filter(|text| !text.is_empty());
        let summarizer = self.small_slot_provider().await;
        let summarizer_window = summarizer
            .model_context_window()
            .await
            .unwrap_or(compaction.fallback_context_window);
        let summary_input_tokens = compaction.max_summary_input_tokens.max(8_000).min(
            summarizer_window
                .saturating_sub(compaction.summary_output_tokens.max(1_024))
                .saturating_sub(2_048),
        );
        let facts = Self::deterministic_compaction_facts(&prefix);
        let transcript_chars = summary_input_tokens
            .saturating_sub(Self::estimate_tokens(&facts))
            .saturating_mul(4)
            .max(compaction.max_transcript_chars);

        // Notes-first compaction (opencode-style): when rolling working notes
        // exist, verify-and-correct them against the transcript instead of a
        // full re-summarization, then keep notes + tail. The tail is the
        // "serialized recent context" opencode retains beside its summary;
        // the transcript is archived either way so nothing durable is lost.
        // A failed verify pass returns the stale notes with `verified=false`,
        // so a stale-notes compaction never claims verification.
        // A manual focus request is honored by the verify pass (it is the
        // only summarization that runs on this path).
        if compaction.notes_enabled
            && compaction.notes_verify_at_compaction
            && existing_notes.is_some()
        {
            let focus_text = focus.map(str::trim).filter(|text| !text.is_empty());
            let (verified, verify_ok) = self
                .verify_working_notes(
                    session,
                    &prefix,
                    compaction,
                    focus_text,
                    transcript_chars,
                    &facts,
                )
                .await;
            let prefix_len = prefix.len();
            let framed_notes = Self::frame_working_notes(&verified);
            let mut compacted_messages = vec![Message::synthetic_assistant(
                framed_notes.clone(),
                "compaction_summary",
            )];
            compacted_messages.extend(tail);
            let post_tokens: usize = compacted_messages.iter().map(Self::message_tokens).sum();
            let post_budget = conversation_budget.unwrap_or_else(|| {
                summarizer_window.saturating_sub(compaction.reserved_output_tokens)
            });
            if post_tokens > post_budget {
                warn!(
                    "Verified-notes checkpoint would remain over budget ({} > {}); preserving history",
                    post_tokens, post_budget
                );
                return Ok(if pruned > 0 {
                    self.session_manager.update_session(session).await?;
                    Some((pruned, 0, false))
                } else {
                    None
                });
            }
            if let Err(error) = self.storage.archive_messages(&session.id, &prefix) {
                warn!(
                    "Failed to archive compacted messages for {}: {}",
                    session.id, error
                );
                return Ok(if pruned > 0 {
                    self.session_manager.update_session(session).await?;
                    Some((pruned, 0, false))
                } else {
                    None
                });
            }
            if let Some(ref mut cs) = session.context_state {
                cs.compaction_stats.estimated_tokens_saved +=
                    prefix_tokens.saturating_sub(Self::estimate_tokens(&framed_notes));
            }
            session.messages = compacted_messages;
            self.store_working_notes(session, verified, verify_ok);
            return Ok(Some((pruned, prefix_len, true)));
        }

        let mut prompt_text = COMPACTION_PROMPT.to_string();
        if let Some(focus) = focus.map(str::trim).filter(|text| !text.is_empty()) {
            prompt_text.push_str(&format!(
                "\n\nThe user asked to focus this compaction on: {}",
                focus
            ));
        }
        let mut compact_messages = vec![Message::system(prompt_text)];
        compact_messages.push(Message::user(format!(
            "Deterministically extracted state (authoritative):\n{}\n\nEarlier conversation transcript:\n{}",
            facts,
            Self::transcript_for_compaction(&prefix, transcript_chars)
        )));

        let mut summary = match summarizer
            .complete(Some(&session.id), &compact_messages, &[])
            .await
        {
            Ok(response) => response.content.filter(|text| !text.trim().is_empty()),
            Err(error) => {
                warn!("Compaction model pass failed: {}", error);
                None
            }
        };
        if let Some(candidate) = summary.as_ref() {
            if !Self::valid_checkpoint_summary(candidate, compaction.summary_output_tokens) {
                warn!("Compaction checkpoint failed schema validation; requesting one correction");
                let correction = vec![
                    Message::system(COMPACTION_PROMPT.to_string()),
                    Message::user(format!(
                        "Rewrite this candidate to obey the required headings exactly, remain under {} tokens, and output only the checkpoint:\n\n{}",
                        compaction.summary_output_tokens, candidate
                    )),
                ];
                summary = summarizer
                    .complete(Some(&session.id), &correction, &[])
                    .await
                    .ok()
                    .and_then(|response| response.content)
                    .filter(|corrected| {
                        Self::valid_checkpoint_summary(corrected, compaction.summary_output_tokens)
                    });
                if summary.is_none() {
                    summary = Some(Self::deterministic_checkpoint(
                        &prefix,
                        compaction.summary_output_tokens.min(2_500),
                    ));
                }
            }
        }
        // Never delete history on a failed summary call: opencode aborts and
        // preserves history, and so do we. Keep prune-only progress if the
        // tail prune did anything.
        let Some(summary) = summary else {
            return Ok(if pruned > 0 {
                self.session_manager.update_session(session).await?;
                Some((pruned, 0, false))
            } else {
                None
            });
        };

        // Shrink validation: a summary must cost fewer tokens than the
        // region it shadows, or the compaction is a loss. Fall back to
        // the deterministic summarizer, which is always smaller.
        let framed = Self::frame_compacted_summary(&summary);
        let framed_tokens = framed.chars().count().div_ceil(4).max(1);
        let final_summary = if framed_tokens >= prefix_tokens {
            warn!(
                "Compaction summary did not shrink ({} est. tokens vs {}): using deterministic fallback",
                framed_tokens, prefix_tokens
            );
            Self::frame_compacted_summary(&Self::deterministic_checkpoint(&prefix, 2_500))
        } else {
            framed
        };

        let mut compacted_messages = vec![Message::synthetic_assistant(
            final_summary.clone(),
            "compaction_summary",
        )];
        compacted_messages.extend(tail);
        let post_tokens: usize = compacted_messages.iter().map(Self::message_tokens).sum();
        let post_budget = conversation_budget
            .unwrap_or_else(|| summarizer_window.saturating_sub(compaction.reserved_output_tokens));
        if post_tokens > post_budget {
            warn!(
                "Compaction checkpoint failed final fit validation ({} > {}); preserving history",
                post_tokens, post_budget
            );
            return Ok(if pruned > 0 {
                self.session_manager.update_session(session).await?;
                Some((pruned, 0, false))
            } else {
                None
            });
        }

        // Preserve what is about to be summarized away so the `sessions`
        // tool can still search and read pre-compaction content. Fail closed
        // like the summary-failure path above: never replace history that
        // was not archived.
        if let Err(error) = self.storage.archive_messages(&session.id, &prefix) {
            warn!(
                "Failed to archive compacted messages for {}: {}",
                session.id, error
            );
            return Ok(if pruned > 0 {
                self.session_manager.update_session(session).await?;
                Some((pruned, 0, false))
            } else {
                None
            });
        }
        if let Some(ref mut cs) = session.context_state {
            cs.compaction_stats.estimated_tokens_saved +=
                prefix_tokens.saturating_sub(Self::estimate_tokens(&final_summary));
        }

        session.messages = compacted_messages;

        Ok(Some((pruned, prefix.len(), true)))
    }

    pub(super) fn frame_working_notes(notes: &str) -> String {
        format!(
            "<compacted-summary>\n# Session Working Notes (verified at compaction)\n{}\n</compacted-summary>\n\nDo not acknowledge the compacted summary explicitly in your reply. Keep the working notes current with `update_notes`.",
            notes.trim()
        )
    }

    pub(super) fn store_working_notes(&self, session: &mut Session, text: String, verified: bool) {
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        let mut state = session.context_state.clone().unwrap_or_default();
        let iteration = state
            .working_notes
            .as_ref()
            .map(|notes| notes.updated_iteration)
            .unwrap_or(0);
        state.working_notes = Some(crate::storage::models::WorkingNotes {
            text: trimmed,
            updated_at: Some(chrono::Utc::now()),
            updated_iteration: iteration + 1,
            verified,
        });
        session.context_state = Some(state);
    }

    pub(super) async fn verify_working_notes(
        &self,
        session: &Session,
        prefix: &[Message],
        compaction: &crate::config::CompactionConfig,
        focus: Option<&str>,
        transcript_chars: usize,
        deterministic_facts: &str,
    ) -> (String, bool) {
        let current = session
            .context_state
            .as_ref()
            .and_then(|state| state.working_notes.as_ref())
            .map(|notes| notes.text.trim().to_string())
            .filter(|text| !text.is_empty())
            .unwrap_or_default();
        if current.is_empty() {
            return (Self::summarize_for_context(prefix, 2_500), false);
        }
        let transcript = Self::transcript_for_compaction(prefix, transcript_chars);
        let mut user_text = format!(
            "Current working notes:\n{}\n\nDeterministically extracted state (authoritative):\n{}\n\nEarlier conversation transcript:\n{}",
            current, deterministic_facts, transcript
        );
        if let Some(focus) = focus.map(str::trim).filter(|text| !text.is_empty()) {
            user_text.push_str(&format!(
                "\n\nThe user asked to focus this compaction on: {}",
                focus
            ));
        }
        let verify_messages = vec![
            Message::system(NOTES_VERIFY_PROMPT.to_string()),
            Message::user(user_text),
        ];
        let provider = self.small_slot_provider().await;
        match provider
            .complete(Some(&session.id), &verify_messages, &[])
            .await
        {
            Ok(response) => {
                let corrected = response
                    .content
                    .unwrap_or_else(|| current.clone())
                    .trim()
                    .to_string();
                if corrected.is_empty() {
                    (current, false)
                } else if corrected.chars().count() > compaction.notes_max_chars {
                    (
                        corrected.chars().take(compaction.notes_max_chars).collect(),
                        true,
                    )
                } else {
                    (corrected, true)
                }
            }
            Err(error) => {
                warn!("Working-notes verification pass failed: {}", error);
                (current, false)
            }
        }
    }

    /// Manual compaction: verify-and-correct the rolling working notes (or
    /// full-summarize when there are no notes), archive the compacted prefix,
    /// and keep notes/summary + recent tail. Refuses while a turn is in
    /// flight for the same reason truncation does.
    pub async fn compact_session_now(
        &self,
        session_id: &str,
        focus: Option<&str>,
    ) -> Result<(usize, usize, bool)> {
        if self.active_runs.contains_key(session_id) {
            return Err(OSAgentError::Session(
                "Cannot compact while the agent is running. Stop it first.".to_string(),
            ));
        }

        let session_lock = self.get_session_lock(session_id);
        let _lock_guard = session_lock.lock().await;
        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| OSAgentError::Session("Session not found".to_string()))?;

        let runtime_config = self.config.read().await.clone();
        let active_workspace = Self::resolve_workspace_for_session(&session, &runtime_config)
            .unwrap_or_else(|| runtime_config.get_active_workspace());

        let outcome = self
            .compact_session_history(
                &mut session,
                &active_workspace,
                &runtime_config.compaction,
                focus,
                None,
            )
            .await?
            .ok_or_else(|| {
                OSAgentError::Session("Nothing to compact yet (history too short).".to_string())
            })?;
        let (pruned, compacted, replayed) = outcome;
        // Refresh the cached context numbers so the context ring drops
        // immediately: the shrunken transcript costs fewer tokens, and the
        // pre-compact provider usage no longer describes the live request.
        let post_tokens: usize = session.messages.iter().map(Self::message_tokens).sum();
        if let Some(ref mut cs) = session.context_state {
            cs.compaction_stats.total_compactions += 1;
            cs.compaction_stats.total_pruned_messages += pruned;
            cs.compaction_stats.total_compacted_messages += compacted;
            cs.estimated_tokens = post_tokens;
            cs.last_request_usage = None;
        }
        self.event_bus.emit(AgentEvent::ContextUpdate {
            session_id: session_id.to_string(),
            sequence: 0,
            context_window: session
                .context_state
                .as_ref()
                .map(|cs| cs.context_window)
                .unwrap_or(runtime_config.compaction.fallback_context_window),
            estimated_tokens: post_tokens,
            budget_tokens: session
                .context_state
                .as_ref()
                .map(|cs| cs.budget_tokens)
                .unwrap_or(runtime_config.compaction.fallback_context_window),
            tool_schema_tokens: 0,
            condensed: true,
            actual_usage: session
                .context_state
                .as_ref()
                .and_then(|cs| cs.actual_usage.clone())
                .map(|u| crate::agent::events::EventTokenUsage {
                    input: u.input,
                    output: u.output,
                    total: u.total,
                    cached_read: u.cached_read,
                    cached_write: u.cached_write,
                    reasoning: u.reasoning,
                    cache_reason: None,
                }),
            last_request_usage: None,
            subagent_session_id: None,
            timestamp: SystemTime::now(),
        });
        self.record_session_event(
            &mut session,
            "compaction",
            serde_json::json!({
                "pruned_messages": pruned,
                "compacted_messages": compacted,
                "replayed": replayed,
                "manual": true,
            }),
        )?;
        self.event_bus.emit(AgentEvent::Compaction {
            session_id: session_id.to_string(),
            sequence: 0,
            pruned_messages: pruned,
            compacted_messages: compacted,
            replayed,
            timestamp: SystemTime::now(),
        });
        self.session_manager.update_session(&session).await?;
        Ok(outcome)
    }
}
