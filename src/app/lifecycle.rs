//! Document transitions kept together, including the refresh after a
//! structural save. Presentation derives its status from this state.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Status { Idle, Opening, Saving, Unavailable }

#[derive(Clone, Copy, Default)]
pub(super) enum Lifecycle {
    #[default]
    Idle,
    Opening,
    Saving { structural: bool },
    Refreshing { generation: u64 },
    Unavailable,
}

impl Lifecycle {
    pub(super) fn status(self) -> Status {
        match self {
            Self::Idle => Status::Idle,
            Self::Opening => Status::Opening,
            Self::Saving { .. } | Self::Refreshing { .. } => Status::Saving,
            Self::Unavailable => Status::Unavailable,
        }
    }

    pub(super) fn start_open(&mut self) { *self = Self::Opening; }
    pub(super) fn start_save(&mut self, structural: bool) { *self = Self::Saving { structural }; }
    pub(super) fn refresh(&mut self, generation: u64) { *self = Self::Refreshing { generation }; }
    pub(super) fn finish(&mut self) { *self = Self::Idle; }
    pub(super) fn unavailable(&mut self) { *self = Self::Unavailable; }
    pub(super) fn is_structural_save(self) -> bool { matches!(self, Self::Saving { structural: true }) }
    pub(super) fn is_refresh(self, generation: u64) -> bool { matches!(self, Self::Refreshing { generation: g } if g == generation) }
}
