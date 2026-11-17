//! Comparisons, variants and tour branches (format version 2): the written parts.
//!
//! A comparison has two or more **sides** (paths of node ids) and **rows** that align
//! their steps by job. A **variant** of a side overrides only the rows that differ. A
//! tour's **branches** use the same [`Variant`] object, keyed by step id instead of row id.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A row's result. Chorong derives `same` and `missing` where it can; otherwise it is the
/// author's claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RowResult {
    Same,
    Differs,
    Missing,
}

/// Where a step sits: setup (numbered S1, S2…) or the call itself (1, 2…).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    Setup,
    Call,
}

/// One side's entry in a row: a step, or why the side has none.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cell {
    /// A node (or external node) id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    /// Why this side has no step for the row ("not reached" in a tour branch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing: Option<String>,
    /// Runtime values at this step, as the author saw them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A variant's replacement for one row (or, in a tour branch, one step).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Override {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Comparisons only: the row's result under this variant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<RowResult>,
    /// Comparisons only: the row's callout under this variant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callout: Option<String>,
}

impl Override {
    /// The cell this override puts in place.
    pub fn cell(&self) -> Cell {
        Cell {
            step: self.step.clone(),
            missing: self.missing.clone(),
            values: self.values.clone(),
            note: self.note.clone(),
        }
    }
}

/// An alternative version of a side (one per tool, scenario or configuration), or a tour
/// branch. Overrides only the rows (or steps) it names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variant {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Row id (comparison) or step id (tour branch) to its replacement.
    pub rows: BTreeMap<String, Override>,
}

/// One path being compared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Side {
    pub id: String,
    pub label: String,
    /// Node ids along this path, first to last.
    pub path: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<Variant>,
}

/// Steps of every side aligned by the job they do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    pub id: String,
    /// Short name of the job, e.g. "Validate input".
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<Phase>,
    /// Side id to that side's entry. Every side has one.
    pub cells: BTreeMap<String, Cell>,
    /// Absent: derived where Chorong can (see the format docs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<RowResult>,
    /// Where the paths split, or anything else worth calling out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}
