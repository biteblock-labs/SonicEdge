#[derive(Debug, Clone)]
pub struct RiskDecision {
    pub pass: bool,
    pub score: u32,
    pub reasons: Vec<String>,
}

impl RiskDecision {
    pub fn pass() -> Self {
        Self {
            pass: true,
            score: 0,
            reasons: Vec::new(),
        }
    }

    pub fn fail(reason: impl Into<String>) -> Self {
        Self {
            pass: false,
            score: 0,
            reasons: vec![reason.into()],
        }
    }

    pub fn add_reason(&mut self, reason: impl Into<String>) {
        self.reasons.push(reason.into());
    }
}
