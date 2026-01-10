/// Represents the state of a search operation
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SearchState {
    /// Search is inactive (no query)
    #[default]
    Off,
    /// Search input is active (user is typing)
    On { query: String },
    /// Search query is applied (not editing, but filter is active)
    Applied { query: String },
}

impl SearchState {
    /// Create a new active search with an empty query
    pub fn new_on() -> Self {
        Self::On {
            query: String::new(),
        }
    }

    /// Create a new active search with the given query
    pub fn with_query(query: String) -> Self {
        Self::On { query }
    }

    /// Check if search is active (on)
    pub fn is_on(&self) -> bool {
        matches!(self, Self::On { .. })
    }

    /// Check if search is inactive (off)
    pub fn is_off(&self) -> bool {
        matches!(self, Self::Off)
    }

    /// Get the search query if search is active or applied, otherwise return empty string
    pub fn query(&self) -> &str {
        match self {
            Self::Off => "",
            Self::On { query } | Self::Applied { query } => query.as_str(),
        }
    }

    /// Get a mutable reference to the query if search is active (not applied)
    pub fn query_mut(&mut self) -> Option<&mut String> {
        match self {
            Self::Off | Self::Applied { .. } => None,
            Self::On { query } => Some(query),
        }
    }

    /// Activate search with empty query
    pub fn activate(&mut self) {
        *self = Self::On {
            query: String::new(),
        };
    }

    /// Apply the current search query (transition from On to Applied)
    /// If already applied or has empty query, deactivates instead
    pub fn apply(&mut self) {
        match self {
            Self::On { query } if !query.is_empty() => {
                let q = std::mem::take(query);
                *self = Self::Applied { query: q };
            }
            _ => {
                *self = Self::Off;
            }
        }
    }

    /// Deactivate search completely (clear query)
    pub fn deactivate(&mut self) {
        *self = Self::Off;
    }

    /// Clear the query but keep search active
    pub fn clear_query(&mut self) {
        if let Self::On { query } = self {
            query.clear();
        }
    }
}
