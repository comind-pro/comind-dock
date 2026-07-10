use std::fmt;

/// Short public pane id, rendered as `%1`, `%2`, …
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaneId(pub u64);

impl fmt::Display for PaneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "%{}", self.0)
    }
}

/// Short public tab id, rendered as `@1`, `@2`, …
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(pub u64);

impl fmt::Display for TabId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}", self.0)
    }
}

/// Short public workspace id, rendered as `#1`, `#2`, …
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkspaceId(pub u64);

impl fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// One shared counter for all id kinds — ids stay short and globally unique.
#[derive(Debug, Default)]
pub struct IdGen {
    next: u64,
}

impl IdGen {
    fn bump(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    pub fn pane(&mut self) -> PaneId {
        PaneId(self.bump())
    }

    pub fn tab(&mut self) -> TabId {
        TabId(self.bump())
    }

    pub fn workspace(&mut self) -> WorkspaceId {
        WorkspaceId(self.bump())
    }
}
