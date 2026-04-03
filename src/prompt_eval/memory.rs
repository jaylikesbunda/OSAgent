use crate::prompt_eval::scorer::AggregateScore;
use crate::prompt_eval::variation::{shuffle_with_rng, PromptConfig};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct EditAtom {
    pub field: usize,
    pub new_value: String,
}

impl EditAtom {
    pub fn new(field: usize, new_value: String) -> Self {
        EditAtom { field, new_value }
    }

    pub fn to_key(&self) -> String {
        format!("{}:{}", self.field, self.new_value)
    }

    pub fn from_key(key: &str) -> Option<Self> {
        let parts: Vec<&str> = key.splitn(2, ':').collect();
        if parts.len() == 2 {
            Some(EditAtom {
                field: parts[0].parse().ok()?,
                new_value: parts[1].to_string(),
            })
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditStats {
    pub count: usize,
    pub mean_delta: f32,
    pub short_term_delta: f32,
    pub long_term_delta: f32,
    pub failure_count: usize,
    pub failure_rate: f32,
    pub last_seen: usize,
    pub recent_deltas: VecDeque<f32>,
    #[serde(default)]
    pub per_test_stats: HashMap<String, PerTestStats>,
}

impl Default for EditStats {
    fn default() -> Self {
        EditStats {
            count: 0,
            mean_delta: 0.0,
            short_term_delta: 0.0,
            long_term_delta: 0.0,
            failure_count: 0,
            failure_rate: 0.0,
            last_seen: 0,
            recent_deltas: VecDeque::new(),
            per_test_stats: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerTestStats {
    pub count: usize,
    pub mean_delta: f32,
    pub success_rate: f32,
}

impl Default for PerTestStats {
    fn default() -> Self {
        PerTestStats {
            count: 0,
            mean_delta: 0.0,
            success_rate: 0.0,
        }
    }
}

impl EditStats {
    pub fn blended_delta(&self, short_weight: f32) -> f32 {
        if self.recent_deltas.is_empty() {
            return self.mean_delta;
        }
        short_weight * self.short_term_delta + (1.0 - short_weight) * self.long_term_delta
    }

    pub fn confidence(&self, k: f32) -> f32 {
        let n = self.count.max(1) as f32;
        n.sqrt() / (n.sqrt() + k)
    }

    pub fn penalty(&self) -> f32 {
        self.failure_rate * 2.0
    }

    pub fn score(&self, short_weight: f32, confidence_k: f32) -> f32 {
        self.confidence(confidence_k) * self.blended_delta(short_weight) - self.penalty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairStats {
    pub atom1: EditAtom,
    pub atom2: EditAtom,
    pub joint_count: usize,
    pub residual_delta: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationRecord {
    pub iteration: usize,
    pub parent_hash: String,
    pub child_hash: String,
    pub atoms: Vec<EditAtom>,
    pub score_delta: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationArchive {
    entries: Vec<MutationRecord>,
    max_size: usize,
}

impl MutationArchive {
    pub fn new(max_size: usize) -> Self {
        MutationArchive {
            entries: Vec::new(),
            max_size,
        }
    }

    pub fn push(&mut self, record: MutationRecord) {
        if self.entries.len() >= self.max_size {
            self.entries.remove(0);
        }
        self.entries.push(record);
    }

    pub fn iter(&self) -> impl Iterator<Item = &MutationRecord> {
        self.entries.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

mod edit_atom_map {
    use super::{EditAtom, EditStats};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::HashMap;

    #[derive(Serialize, Deserialize)]
    struct Entry {
        key: String,
        value: EditStats,
    }

    pub fn serialize<S>(
        map: &HashMap<EditAtom, EditStats>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let entries: Vec<Entry> = map
            .iter()
            .map(|(k, v)| Entry {
                key: k.to_key(),
                value: v.clone(),
            })
            .collect();
        entries.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<HashMap<EditAtom, EditStats>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let entries: Vec<Entry> = Vec::deserialize(deserializer)?;
        let mut map = HashMap::new();
        for entry in entries {
            if let Some(key) = EditAtom::from_key(&entry.key) {
                map.insert(key, entry.value);
            }
        }
        Ok(map)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedPolicy {
    #[serde(with = "edit_atom_map")]
    pub edit_stats: HashMap<EditAtom, EditStats>,
    pub pair_stats: HashMap<String, PairStats>,
    pub archive: MutationArchive,
    pub short_window: usize,
    pub long_window: usize,
    pub pair_support_threshold: usize,
    pub pair_lift_threshold: f32,
    pub confidence_k: f32,
    pub use_decay: bool,
    pub decay_rate: f32,
    pub short_weight: f32,
}

impl LearnedPolicy {
    pub fn new(short_window: usize, long_window: usize, confidence_k: f32) -> Self {
        LearnedPolicy {
            edit_stats: HashMap::new(),
            pair_stats: HashMap::new(),
            archive: MutationArchive::new(1000),
            short_window,
            long_window,
            pair_support_threshold: 10,
            pair_lift_threshold: 0.05,
            confidence_k,
            use_decay: true,
            decay_rate: 0.995,
            short_weight: 0.4,
        }
    }

    pub fn record_mutation(
        &mut self,
        iteration: usize,
        parent: &PromptConfig,
        child: &PromptConfig,
        parent_score: f32,
        child_score: f32,
        parent_test_scores: Option<&HashMap<String, TestScore>>,
        child_test_scores: Option<&HashMap<String, TestScore>>,
    ) {
        let atoms = Self::compute_atoms(parent, child);
        let delta = child_score - parent_score;
        let parent_hash = parent.hash_key();
        let child_hash = child.hash_key();

        self.archive.push(MutationRecord {
            iteration,
            parent_hash: parent_hash.clone(),
            child_hash: child_hash.clone(),
            atoms: atoms.clone(),
            score_delta: delta,
        });

        let delta_per_atom = if atoms.is_empty() {
            0.0
        } else {
            delta / atoms.len() as f32
        };

        for atom in &atoms {
            let stats = self.edit_stats.entry(atom.clone()).or_default();

            if stats.recent_deltas.len() >= self.short_window {
                stats.recent_deltas.pop_front();
            }
            stats.recent_deltas.push_back(delta_per_atom);

            stats.count += 1;
            stats.mean_delta =
                (stats.mean_delta * (stats.count - 1) as f32 + delta_per_atom) / stats.count as f32;
            stats.short_term_delta = if stats.recent_deltas.is_empty() {
                0.0
            } else {
                stats.recent_deltas.iter().sum::<f32>() / stats.recent_deltas.len() as f32
            };

            if delta < 0.0 {
                stats.failure_count += 1;
            }
            stats.failure_rate = stats.failure_count as f32 / stats.count as f32;
            stats.last_seen = iteration;

            // EMA for long_term_delta - always updates
            let alpha = 2.0 / (self.long_window as f32 + 1.0);
            stats.long_term_delta = alpha * delta_per_atom + (1.0 - alpha) * stats.long_term_delta;

            // Track per-test deltas
            if let (Some(parent_scores), Some(child_scores)) =
                (parent_test_scores, child_test_scores)
            {
                for (test_name, child_ts) in child_scores {
                    if let Some(parent_ts) = parent_scores.get(test_name) {
                        let per_test_entry =
                            stats.per_test_stats.entry(test_name.clone()).or_default();

                        let test_delta =
                            (child_ts.correctness + child_ts.efficiency + child_ts.safety) / 3.0
                                - (parent_ts.correctness + parent_ts.efficiency + parent_ts.safety)
                                    / 3.0;

                        per_test_entry.count += 1;
                        per_test_entry.mean_delta = (per_test_entry.mean_delta
                            * (per_test_entry.count - 1) as f32
                            + test_delta)
                            / per_test_entry.count as f32;
                        per_test_entry.success_rate = (per_test_entry.success_rate
                            * (per_test_entry.count - 1) as f32
                            + if test_delta > 0.0 { 1.0 } else { 0.0 })
                            / per_test_entry.count as f32;
                    }
                }
            }
        }

        // Track ALL pairs of atoms, not just when exactly 2
        for i in 0..atoms.len() {
            for j in (i + 1)..atoms.len() {
                self.update_pair_stats(&atoms[i], &atoms[j], delta);
            }
        }
    }

    fn update_long_term_delta(&mut self, _atom: &EditAtom, _delta: f32) {
        // Deprecated - EMA now calculated inline in record_mutation
    }

    fn pair_key(atom1: &EditAtom, atom2: &EditAtom) -> String {
        if atom1.field < atom2.field {
            format!(
                "{}:{}|{}:{}",
                atom1.field, atom1.new_value, atom2.field, atom2.new_value
            )
        } else {
            format!(
                "{}:{}|{}:{}",
                atom2.field, atom2.new_value, atom1.field, atom1.new_value
            )
        }
    }

    fn update_pair_stats(&mut self, atom1: &EditAtom, atom2: &EditAtom, delta: f32) {
        let key = Self::pair_key(atom1, atom2);

        let stats1 = self
            .edit_stats
            .get(atom1)
            .map(|s| s.mean_delta)
            .unwrap_or(0.0);
        let stats2 = self
            .edit_stats
            .get(atom2)
            .map(|s| s.mean_delta)
            .unwrap_or(0.0);
        let expected_joint = stats1 + stats2;
        let residual = delta - expected_joint;

        let pair_entry = self
            .pair_stats
            .entry(key.clone())
            .or_insert_with(|| PairStats {
                atom1: atom1.clone(),
                atom2: atom2.clone(),
                joint_count: 0,
                residual_delta: 0.0,
            });

        pair_entry.joint_count += 1;
        if pair_entry.joint_count > 1 {
            pair_entry.residual_delta =
                (pair_entry.residual_delta * (pair_entry.joint_count - 1) as f32 + residual)
                    / pair_entry.joint_count as f32;
        } else {
            pair_entry.residual_delta = residual;
        }
    }

    pub fn credit_based_mutate(
        &self,
        child: &mut PromptConfig,
        parent: &PromptConfig,
        rng: &mut StdRng,
    ) {
        if self.edit_stats.is_empty() {
            Self::random_mutate(child, rng, 2);
            return;
        }

        // Score atoms considering pair synergies
        let mut atom_scores: Vec<(EditAtom, f32)> = self
            .edit_stats
            .iter()
            .filter(|(_, stats)| stats.count > 0)
            .map(|(atom, stats)| {
                let base_score = stats.score(self.short_weight, self.confidence_k);
                (atom.clone(), base_score)
            })
            .collect();

        atom_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let num_to_copy = rng.gen_range(1..=2);
        let mut copied = 0;
        let mut chosen_atoms: Vec<EditAtom> = Vec::new();

        for (atom, score) in &atom_scores {
            if copied >= num_to_copy {
                break;
            }

            if *score <= 0.0 {
                continue;
            }

            if Self::get_field_value(parent, atom.field) == atom.new_value {
                continue;
            }

            let prob = score.clamp(0.0, 1.0);

            if rng.gen::<f32>() < prob {
                Self::set_field_from_atom(child, atom);
                chosen_atoms.push(atom.clone());
                copied += 1;
            }
        }

        // Use pair stats: if we chose one atom, boost a synergistic partner
        if chosen_atoms.len() == 1 && rng.gen::<f32>() < 0.4 {
            if let Some(partner) = self.find_synergistic_partner(&chosen_atoms[0], parent) {
                Self::set_field_from_atom(child, &partner);
            }
        }

        Self::random_mutate(child, rng, 1);
    }

    fn find_synergistic_partner(&self, atom: &EditAtom, parent: &PromptConfig) -> Option<EditAtom> {
        let mut best_partner: Option<EditAtom> = None;
        let mut best_lift = 0.0f32;

        for (key, pair) in &self.pair_stats {
            if pair.joint_count < self.pair_support_threshold {
                continue;
            }

            let is_match =
                (pair.atom1.to_key() == atom.to_key()) || (pair.atom2.to_key() == atom.to_key());

            if !is_match {
                continue;
            }

            let partner = if pair.atom1.to_key() == atom.to_key() {
                &pair.atom2
            } else {
                &pair.atom1
            };

            // Skip if parent already has this value
            if Self::get_field_value(parent, partner.field) == partner.new_value {
                continue;
            }

            // Positive residual = synergy (better together than expected)
            if pair.residual_delta > best_lift && pair.residual_delta > self.pair_lift_threshold {
                best_lift = pair.residual_delta;
                best_partner = Some(partner.clone());
            }
        }

        best_partner
    }

    /// Find edits that specifically help a given test
    pub fn find_edits_for_test(&self, test_name: &str, min_count: usize) -> Vec<(EditAtom, f32)> {
        let mut results: Vec<(EditAtom, f32)> = Vec::new();

        for (atom, stats) in &self.edit_stats {
            if stats.count < min_count {
                continue;
            }

            if let Some(per_test) = stats.per_test_stats.get(test_name) {
                if per_test.count >= min_count && per_test.mean_delta > 0.0 {
                    // Weight by success rate and mean delta
                    let score = per_test.mean_delta * per_test.success_rate;
                    results.push((atom.clone(), score));
                }
            }
        }

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results
    }

    /// Get summary of which edits help/hurt specific test categories
    pub fn analyze_test_patterns(&self) -> HashMap<String, Vec<(String, f32)>> {
        let mut analysis: HashMap<String, Vec<(String, f32)>> = HashMap::new();

        for (atom, stats) in &self.edit_stats {
            for (test_name, per_test) in &stats.per_test_stats {
                if per_test.count < 3 {
                    continue;
                }
                let entry = analysis.entry(test_name.clone()).or_default();
                entry.push((atom.to_key(), per_test.mean_delta));
            }
        }

        for (_test, entries) in analysis.iter_mut() {
            entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        }

        analysis
    }

    fn get_field_value(config: &PromptConfig, field: usize) -> String {
        let val = match field {
            0 => format!("{:?}", config.identity_style),
            1 => format!("{:?}", config.verbosity),
            2 => format!("{:?}", config.tone),
            3 => format!("{:?}", config.tool_guidance),
            4 => format!("{:?}", config.priority_style),
            5 => format!("{:?}", config.workflow_style),
            6 => config.include_ui_section.to_string(),
            7 => config.include_codebase_nav.to_string(),
            8 => config.include_parallel_tools.to_string(),
            9 => config
                .section_order
                .iter()
                .map(|s| format!("{:?}", s))
                .collect::<Vec<_>>()
                .join(","),
            10 => format!("{:?}", config.decision_strategy),
            11 => format!("{:?}", config.context_behavior),
            12 => format!("{:?}", config.validation_style),
            13 => format!("{:?}", config.response_brevity),
            14 => format!("{:?}", config.retry_philosophy),
            15 => format!("{:?}", config.tool_philosophy),
            16 => format!("{:?}", config.identity_variant),
            17 => format!("{:?}", config.priorities_variant),
            18 => format!("{:?}", config.safety_variant),
            19 => format!("{:?}", config.workflow_variant),
            20 => format!("{:?}", config.communication_variant),
            _ => return String::new(),
        };
        val
    }

    fn set_field_from_atom(config: &mut PromptConfig, atom: &EditAtom) {
        match atom.field {
            0 => {
                config.identity_style = match atom.new_value.as_str() {
                    "Formal" => IdentityStyle::Formal,
                    "Casual" => IdentityStyle::Casual,
                    "Technical" => IdentityStyle::Technical,
                    "Concise" => IdentityStyle::Concise,
                    "Operator" => IdentityStyle::Operator,
                    "Helpful" => IdentityStyle::Helpful,
                    _ => return,
                }
            }
            1 => {
                config.verbosity = match atom.new_value.as_str() {
                    "Minimal" => Verbosity::Minimal,
                    "Normal" => Verbosity::Normal,
                    "Detailed" => Verbosity::Detailed,
                    "UltraDetailed" => Verbosity::UltraDetailed,
                    _ => return,
                }
            }
            2 => {
                config.tone = match atom.new_value.as_str() {
                    "Dry" => Tone::Dry,
                    "Friendly" => Tone::Friendly,
                    "Witty" => Tone::Witty,
                    "Direct" => Tone::Direct,
                    "Calm" => Tone::Calm,
                    "Assertive" => Tone::Assertive,
                    _ => return,
                }
            }
            3 => {
                config.tool_guidance = match atom.new_value.as_str() {
                    "None" => ToolGuidance::None,
                    "Brief" => ToolGuidance::Brief,
                    "Normal" => ToolGuidance::Normal,
                    "Extended" => ToolGuidance::Extended,
                    "WithExamples" => ToolGuidance::WithExamples,
                    _ => return,
                }
            }
            4 => {
                config.priority_style = match atom.new_value.as_str() {
                    "Efficiency" => PriorityStyle::Efficiency,
                    "Safety" => PriorityStyle::Safety,
                    "Thoroughness" => PriorityStyle::Thoroughness,
                    "Balanced" => PriorityStyle::Balanced,
                    _ => return,
                }
            }
            5 => {
                config.workflow_style = match atom.new_value.as_str() {
                    "Linear" => WorkflowStyle::Linear,
                    "Exploratory" => WorkflowStyle::Exploratory,
                    "Parallel" => WorkflowStyle::Parallel,
                    "Todo" => WorkflowStyle::Todo,
                    _ => return,
                }
            }
            6 => config.include_ui_section = atom.new_value.parse().unwrap_or(true),
            7 => config.include_codebase_nav = atom.new_value.parse().unwrap_or(true),
            8 => config.include_parallel_tools = atom.new_value.parse().unwrap_or(true),
            9 => config.section_order = Self::parse_section_order(&atom.new_value),
            10 => {
                config.decision_strategy = match atom.new_value.as_str() {
                    "ActFirst" => DecisionStrategy::ActFirst,
                    "ExploreThenAct" => DecisionStrategy::ExploreThenAct,
                    "ParallelProbe" => DecisionStrategy::ParallelProbe,
                    _ => return,
                }
            }
            11 => {
                config.context_behavior = match atom.new_value.as_str() {
                    "ReadBeforeAct" => ContextBehavior::ReadBeforeAct,
                    "ActFirst" => ContextBehavior::ActFirst,
                    "MinimalContext" => ContextBehavior::MinimalContext,
                    _ => return,
                }
            }
            12 => {
                config.validation_style = match atom.new_value.as_str() {
                    "Thorough" => ValidationStyle::Thorough,
                    "QuickCheck" => ValidationStyle::QuickCheck,
                    "None" => ValidationStyle::None,
                    _ => return,
                }
            }
            13 => {
                config.response_brevity = match atom.new_value.as_str() {
                    "Detailed" => ResponseBrevity::Detailed,
                    "Concise" => ResponseBrevity::Concise,
                    "Minimal" => ResponseBrevity::Minimal,
                    _ => return,
                }
            }
            14 => {
                config.retry_philosophy = match atom.new_value.as_str() {
                    "Retry3x" => RetryPhilosophy::Retry3x,
                    "Retry1x" => RetryPhilosophy::Retry1x,
                    "NoRetry" => RetryPhilosophy::NoRetry,
                    _ => return,
                }
            }
            15 => {
                config.tool_philosophy = match atom.new_value.as_str() {
                    "UseToolsLiberally" => ToolPhilosophy::UseToolsLiberally,
                    "UseToolsSparingly" => ToolPhilosophy::UseToolsSparingly,
                    _ => return,
                }
            }
            16 => {
                config.identity_variant = match atom.new_value.as_str() {
                    "Standard" => IdentityVariant::Standard,
                    "Minimal" => IdentityVariant::Minimal,
                    "Technical" => IdentityVariant::Technical,
                    "Casual" => IdentityVariant::Casual,
                    "Detailed" => IdentityVariant::Detailed,
                    "Gpt5Efficient" => IdentityVariant::Gpt5Efficient,
                    "ClaudeStyle" => IdentityVariant::ClaudeStyle,
                    _ => return,
                }
            }
            17 => {
                config.priorities_variant = match atom.new_value.as_str() {
                    "Standard" => PrioritiesVariant::Standard,
                    "Minimal" => PrioritiesVariant::Minimal,
                    "Detailed" => PrioritiesVariant::Detailed,
                    "Efficiency" => PrioritiesVariant::Efficiency,
                    "Safety" => PrioritiesVariant::Safety,
                    "Quality" => PrioritiesVariant::Quality,
                    "Gpt5Efficient" => PrioritiesVariant::Gpt5Efficient,
                    "DirectKnowledge" => PrioritiesVariant::DirectKnowledge,
                    _ => return,
                }
            }
            18 => {
                config.safety_variant = match atom.new_value.as_str() {
                    "Standard" => SafetyVariant::Standard,
                    "Minimal" => SafetyVariant::Minimal,
                    "Detailed" => SafetyVariant::Detailed,
                    "Paranoid" => SafetyVariant::Paranoid,
                    "Trusting" => SafetyVariant::Trusting,
                    "RuleBased" => SafetyVariant::RuleBased,
                    "InjectionDefense" => SafetyVariant::InjectionDefense,
                    _ => return,
                }
            }
            19 => {
                config.workflow_variant = match atom.new_value.as_str() {
                    "Standard" => WorkflowVariant::Standard,
                    "Minimal" => WorkflowVariant::Minimal,
                    "Detailed" => WorkflowVariant::Detailed,
                    "ActFirst" => WorkflowVariant::ActFirst,
                    "ExploreFirst" => WorkflowVariant::ExploreFirst,
                    "ParallelFirst" => WorkflowVariant::ParallelFirst,
                    "StepByStep" => WorkflowVariant::StepByStep,
                    "Efficient" => WorkflowVariant::Efficient,
                    "Gpt5Thinking" => WorkflowVariant::Gpt5Thinking,
                    "IterativeBuild" => WorkflowVariant::IterativeBuild,
                    "ContextDriven" => WorkflowVariant::ContextDriven,
                    _ => return,
                }
            }
            20 => {
                config.communication_variant = match atom.new_value.as_str() {
                    "Standard" => CommunicationVariant::Standard,
                    "Minimal" => CommunicationVariant::Minimal,
                    "Detailed" => CommunicationVariant::Detailed,
                    "Terse" => CommunicationVariant::Terse,
                    "Conversational" => CommunicationVariant::Conversational,
                    "Technical" => CommunicationVariant::Technical,
                    "Gpt5Style" => CommunicationVariant::Gpt5Style,
                    "ClaudeStyle" => CommunicationVariant::ClaudeStyle,
                    "NaturalProse" => CommunicationVariant::NaturalProse,
                    _ => return,
                }
            }
            _ => {}
        }
    }

    fn parse_section_order(val: &str) -> Vec<crate::prompt_eval::variation::Section> {
        use crate::prompt_eval::variation::Section;
        val.split(',')
            .filter_map(|s| match s.trim() {
                "Identity" => Some(Section::Identity),
                "DateTime" => Some(Section::DateTime),
                "Priorities" => Some(Section::Priorities),
                "Safety" => Some(Section::Safety),
                "Workflow" => Some(Section::Workflow),
                "ToolSelection" => Some(Section::ToolSelection),
                "Communication" => Some(Section::Communication),
                "UI" => Some(Section::UI),
                "CodebaseNav" => Some(Section::CodebaseNav),
                "ParallelTools" => Some(Section::ParallelTools),
                "EditingRules" => Some(Section::EditingRules),
                "Validation" => Some(Section::Validation),
                _ => None,
            })
            .collect()
    }

    pub fn compute_atoms(parent: &PromptConfig, child: &PromptConfig) -> Vec<EditAtom> {
        let mut atoms = Vec::new();

        if parent.identity_style != child.identity_style {
            atoms.push(EditAtom::new(0, format!("{:?}", child.identity_style)));
        }
        if parent.verbosity != child.verbosity {
            atoms.push(EditAtom::new(1, format!("{:?}", child.verbosity)));
        }
        if parent.tone != child.tone {
            atoms.push(EditAtom::new(2, format!("{:?}", child.tone)));
        }
        if parent.tool_guidance != child.tool_guidance {
            atoms.push(EditAtom::new(3, format!("{:?}", child.tool_guidance)));
        }
        if parent.priority_style != child.priority_style {
            atoms.push(EditAtom::new(4, format!("{:?}", child.priority_style)));
        }
        if parent.workflow_style != child.workflow_style {
            atoms.push(EditAtom::new(5, format!("{:?}", child.workflow_style)));
        }
        if parent.include_ui_section != child.include_ui_section {
            atoms.push(EditAtom::new(6, child.include_ui_section.to_string()));
        }
        if parent.include_codebase_nav != child.include_codebase_nav {
            atoms.push(EditAtom::new(7, child.include_codebase_nav.to_string()));
        }
        if parent.include_parallel_tools != child.include_parallel_tools {
            atoms.push(EditAtom::new(8, child.include_parallel_tools.to_string()));
        }
        if parent.section_order != child.section_order {
            atoms.push(EditAtom::new(
                9,
                child
                    .section_order
                    .iter()
                    .map(|s| format!("{:?}", s))
                    .collect::<Vec<_>>()
                    .join(","),
            ));
        }
        if parent.decision_strategy != child.decision_strategy {
            atoms.push(EditAtom::new(10, format!("{:?}", child.decision_strategy)));
        }
        if parent.context_behavior != child.context_behavior {
            atoms.push(EditAtom::new(11, format!("{:?}", child.context_behavior)));
        }
        if parent.validation_style != child.validation_style {
            atoms.push(EditAtom::new(12, format!("{:?}", child.validation_style)));
        }
        if parent.response_brevity != child.response_brevity {
            atoms.push(EditAtom::new(13, format!("{:?}", child.response_brevity)));
        }
        if parent.retry_philosophy != child.retry_philosophy {
            atoms.push(EditAtom::new(14, format!("{:?}", child.retry_philosophy)));
        }
        if parent.tool_philosophy != child.tool_philosophy {
            atoms.push(EditAtom::new(15, format!("{:?}", child.tool_philosophy)));
        }
        if parent.identity_variant != child.identity_variant {
            atoms.push(EditAtom::new(16, format!("{:?}", child.identity_variant)));
        }
        if parent.priorities_variant != child.priorities_variant {
            atoms.push(EditAtom::new(17, format!("{:?}", child.priorities_variant)));
        }
        if parent.safety_variant != child.safety_variant {
            atoms.push(EditAtom::new(18, format!("{:?}", child.safety_variant)));
        }
        if parent.workflow_variant != child.workflow_variant {
            atoms.push(EditAtom::new(19, format!("{:?}", child.workflow_variant)));
        }
        if parent.communication_variant != child.communication_variant {
            atoms.push(EditAtom::new(
                20,
                format!("{:?}", child.communication_variant),
            ));
        }

        atoms
    }

    fn random_mutate(config: &mut PromptConfig, rng: &mut StdRng, count: usize) {
        for _ in 0..count {
            match rng.gen_range(0..22) {
                0 => {
                    if let Some(v) = IdentityStyle::all().as_slice().choose(rng) {
                        config.identity_style = *v;
                    }
                }
                1 => {
                    if let Some(v) = Verbosity::all().as_slice().choose(rng) {
                        config.verbosity = *v;
                    }
                }
                2 => {
                    if let Some(v) = Tone::all().as_slice().choose(rng) {
                        config.tone = *v;
                    }
                }
                3 => {
                    if let Some(v) = PriorityStyle::all().as_slice().choose(rng) {
                        config.priority_style = *v;
                    }
                }
                4 => {
                    if let Some(v) = WorkflowStyle::all().as_slice().choose(rng) {
                        config.workflow_style = *v;
                    }
                }
                5 => {
                    if let Some(v) = ToolGuidance::all().as_slice().choose(rng) {
                        config.tool_guidance = *v;
                    }
                }
                6 => config.include_ui_section = !config.include_ui_section,
                7 => config.include_codebase_nav = !config.include_codebase_nav,
                8 => config.include_parallel_tools = !config.include_parallel_tools,
                9 => {
                    let mut order = config.section_order.clone();
                    shuffle_with_rng(&mut order, rng);
                    config.section_order = order;
                }
                10 => {
                    if let Some(v) = DecisionStrategy::all().as_slice().choose(rng) {
                        config.decision_strategy = *v;
                    }
                }
                11 => {
                    if let Some(v) = ContextBehavior::all().as_slice().choose(rng) {
                        config.context_behavior = *v;
                    }
                }
                12 => {
                    if let Some(v) = ValidationStyle::all().as_slice().choose(rng) {
                        config.validation_style = *v;
                    }
                }
                13 => {
                    if let Some(v) = ResponseBrevity::all().as_slice().choose(rng) {
                        config.response_brevity = *v;
                    }
                }
                14 => {
                    if let Some(v) = RetryPhilosophy::all().as_slice().choose(rng) {
                        config.retry_philosophy = *v;
                    }
                }
                15 => {
                    if let Some(v) = ToolPhilosophy::all().as_slice().choose(rng) {
                        config.tool_philosophy = *v;
                    }
                }
                16 => {
                    if let Some(v) = IdentityVariant::all().as_slice().choose(rng) {
                        config.identity_variant = *v;
                    }
                }
                17 => {
                    if let Some(v) = PrioritiesVariant::all().as_slice().choose(rng) {
                        config.priorities_variant = *v;
                    }
                }
                18 => {
                    if let Some(v) = SafetyVariant::all().as_slice().choose(rng) {
                        config.safety_variant = *v;
                    }
                }
                19 => {
                    if let Some(v) = WorkflowVariant::all().as_slice().choose(rng) {
                        config.workflow_variant = *v;
                    }
                }
                20 => {
                    if let Some(v) = CommunicationVariant::all().as_slice().choose(rng) {
                        config.communication_variant = *v;
                    }
                }
                _ => {}
            }
        }
    }

    pub fn has_sufficient_history(&self) -> bool {
        !self.edit_stats.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestScore {
    pub correctness: f32,
    pub tool_accuracy: f32,
    pub efficiency: f32,
    pub safety: f32,
    pub format: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuccessEntry {
    pub config: PromptConfig,
    pub config_hash: String,
    pub aggregate_score: f32,
    pub test_scores: HashMap<String, TestScore>,
}

impl SuccessEntry {
    pub fn new(
        config: PromptConfig,
        config_hash: String,
        aggregate_score: f32,
        test_scores: HashMap<String, TestScore>,
    ) -> Self {
        SuccessEntry {
            config,
            config_hash,
            aggregate_score,
            test_scores,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    pub enabled: bool,
    pub threshold: f32,
    pub max_entries: usize,
    pub guided_mutation_rate: f32,
    pub random_mutation_min: usize,
    pub random_mutation_max: usize,
    pub policy_top_k: usize,
    pub short_window: usize,
    pub long_window: usize,
    pub confidence_k: f32,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        MemoryConfig {
            enabled: true,
            threshold: 0.6,
            max_entries: 100,
            guided_mutation_rate: 0.7,
            random_mutation_min: 1,
            random_mutation_max: 2,
            policy_top_k: 5,
            short_window: 20,
            long_window: 100,
            confidence_k: 5.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuccessMemory {
    entries: Vec<SuccessEntry>,
    test_index: HashMap<String, Vec<usize>>,
    pub config: MemoryConfig,
    pub policy: LearnedPolicy,
}

impl SuccessMemory {
    pub fn new(config: MemoryConfig) -> Self {
        SuccessMemory {
            entries: Vec::new(),
            test_index: HashMap::new(),
            config: config.clone(),
            policy: LearnedPolicy::new(
                config.short_window,
                config.long_window,
                config.confidence_k,
            ),
        }
    }

    pub fn load(path: &std::path::Path) -> Option<Self> {
        let data = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&data).ok()
    }

    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        let data = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, data)
    }

    pub fn add(&mut self, entry: SuccessEntry) {
        if self.entries.len() >= self.config.max_entries {
            self.evict_oldest();
        }
        let idx = self.entries.len();
        self.entries.push(entry);

        for (test_name, score) in &self.entries[idx].test_scores {
            if score.correctness >= self.config.threshold {
                self.test_index
                    .entry(test_name.clone())
                    .or_default()
                    .push(idx);
            }
        }
    }

    fn evict_oldest(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        let oldest_idx = 0;
        self.entries.remove(oldest_idx);

        for indices in self.test_index.values_mut() {
            if let Some(pos) = indices.iter().position(|&x| x == oldest_idx) {
                indices.remove(pos);
            }
            for idx in indices.iter_mut() {
                if *idx > oldest_idx {
                    *idx -= 1;
                }
            }
        }
    }

    pub fn get_top_n(&self, n: usize) -> Vec<&SuccessEntry> {
        let mut entries: Vec<_> = self.entries.iter().collect();
        entries.sort_by(|a, b| {
            b.aggregate_score
                .partial_cmp(&a.aggregate_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        entries.truncate(n);
        entries
    }

    pub fn weighted_sample<'a>(&'a self, rng: &mut StdRng) -> Option<&'a SuccessEntry> {
        if self.entries.is_empty() {
            return None;
        }

        let weights: Vec<f32> = self
            .entries
            .iter()
            .map(|e| e.aggregate_score.max(0.01).powi(2))
            .collect();

        let total_weight: f32 = weights.iter().sum();
        if total_weight <= 0.0 {
            return self.entries.as_slice().choose(rng).map(|e| e);
        }

        let mut r: f32 = rng.gen_range(0.0..total_weight);
        for (i, w) in weights.iter().enumerate() {
            r -= w;
            if r <= 0.0 {
                return self.entries.get(i);
            }
        }

        self.entries.as_slice().choose(rng).map(|e| e)
    }

    pub fn guided_mutate(&self, child: &mut PromptConfig, parent: &PromptConfig, rng: &mut StdRng) {
        let use_guided = rng.gen::<f32>() < self.config.guided_mutation_rate;

        if use_guided && self.policy.has_sufficient_history() {
            self.policy.credit_based_mutate(child, parent, rng);
        } else {
            let count =
                rng.gen_range(self.config.random_mutation_min..=self.config.random_mutation_max);
            LearnedPolicy::random_mutate(child, rng, count);
        }
    }

    /// Mutate targeting specific struggling tests
    pub fn targeted_mutate(
        &self,
        child: &mut PromptConfig,
        parent: &PromptConfig,
        failing_tests: &[String],
        rng: &mut StdRng,
    ) {
        if failing_tests.is_empty() || !self.policy.has_sufficient_history() {
            self.guided_mutate(child, parent, rng);
            return;
        }

        // Collect edits that help the failing tests
        let mut helpful_edits: Vec<(EditAtom, f32)> = Vec::new();
        for test_name in failing_tests {
            let edits = self.policy.find_edits_for_test(test_name, 3);
            helpful_edits.extend(edits);
        }

        if helpful_edits.is_empty() {
            self.guided_mutate(child, parent, rng);
            return;
        }

        // Sort by score and pick top edits
        helpful_edits.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut applied = 0;
        for (atom, _score) in helpful_edits {
            if applied >= 2 {
                break;
            }
            if LearnedPolicy::get_field_value(parent, atom.field) != atom.new_value {
                LearnedPolicy::set_field_from_atom(child, &atom);
                applied += 1;
            }
        }

        // Still do one random mutation for exploration
        LearnedPolicy::random_mutate(child, rng, 1);
    }

    /// Get analysis of which edits help which tests
    pub fn get_test_analysis(&self) -> HashMap<String, Vec<(String, f32)>> {
        self.policy.analyze_test_patterns()
    }

    pub fn record_mutation(
        &mut self,
        iteration: usize,
        parent: &PromptConfig,
        child: &PromptConfig,
        parent_score: f32,
        child_score: f32,
        parent_test_scores: Option<&HashMap<String, TestScore>>,
        child_test_scores: Option<&HashMap<String, TestScore>>,
    ) {
        self.policy.record_mutation(
            iteration,
            parent,
            child,
            parent_score,
            child_score,
            parent_test_scores,
            child_test_scores,
        );
    }

    pub fn has_sufficient_history(&self) -> bool {
        self.policy.has_sufficient_history()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

use crate::prompt_eval::variation::{
    CommunicationVariant, ContextBehavior, DecisionStrategy, IdentityStyle, IdentityVariant,
    PrioritiesVariant, PriorityStyle, ResponseBrevity, RetryPhilosophy, SafetyVariant, Tone,
    ToolGuidance, ToolPhilosophy, ValidationStyle, Verbosity, WorkflowStyle, WorkflowVariant,
};
