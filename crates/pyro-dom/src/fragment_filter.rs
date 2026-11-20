//! Permissive SanitizerConfig rules applied by the HTML fragment tree builder.
use crate::{LocalName, Namespace, QualName};
use rustc_hash::FxHashSet;

pub type Names = FxHashSet<(Namespace, LocalName)>;

pub struct FragmentFilter {
    pub remove_elements: Names,
    pub remove_attributes: Names,
    pub comments: bool,
    pub processing_instructions: Option<FxHashSet<String>>,
    pub remove_processing_instructions: FxHashSet<String>,
}

impl Default for FragmentFilter {
    fn default() -> Self {
        Self {
            remove_elements: Names::default(),
            remove_attributes: Names::default(),
            comments: true,
            processing_instructions: None,
            remove_processing_instructions: FxHashSet::default(),
        }
    }
}

impl FragmentFilter {
    pub(crate) fn removes_element(&self, name: &QualName) -> bool {
        self.remove_elements.contains(&(name.ns.clone(), name.local.clone()))
    }

    pub(crate) fn allows_attribute(&self, name: &QualName) -> bool {
        !self.remove_attributes.contains(&(name.ns.clone(), name.local.clone()))
    }

    pub(crate) fn allows_pi(&self, target: &str) -> bool {
        self.processing_instructions
            .as_ref()
            .map_or_else(|| !self.remove_processing_instructions.contains(target), |allowed| allowed.contains(target))
    }
}
