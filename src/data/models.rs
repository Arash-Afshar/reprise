use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryFile {
    #[serde(default = "default_version")]
    pub version: u32,
    pub games: Vec<Game>,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub my_username: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Game {
    pub id: String,
    pub game_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub white: String,
    pub black: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub white_elo: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub black_elo: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_control: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eco: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub termination: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub played_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opponent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub my_username: Option<String>,
    pub pgn: String,
    /// Final position snapshot for list preview (avoids replaying PGN on the UI thread).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_fen: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ply: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_uci: Option<String>,
    /// Set by db-index / lite load — never requires parsing analysis blobs.
    #[serde(default)]
    pub has_analysis: bool,
    /// Opened on the playbench for review (with analysis). Not set by list `a` / analyze-all alone.
    #[serde(default)]
    pub reviewed: bool,
    /// Heavy; omitted from db-index.json. Prefer lazy load when needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analysis: Option<GameAnalysis>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameAnalysis {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analyzed_at: Option<String>,
    pub moves: Vec<AnalyzedMove>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzedMove {
    pub ply: u32,
    pub san: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uci: Option<String>,
    pub color: String,
    pub fen_before: String,
    pub fen_after: String,
    pub classification: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loss_cp: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval_played_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval_before_best_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval_white_best: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval_white_played: Option<f64>,
    #[serde(default)]
    pub best_moves: Vec<BestMove>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teaching: Option<Teaching>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ideal_after_played: Option<PlayedLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BestMove {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uci: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub san: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pv_san: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayedLine {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pv_san: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Teaching {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why_played: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why_best: Option<String>,
}

impl Game {
    pub fn title(&self) -> String {
        if let Some(opp) = self.opponent.as_deref().filter(|s| !s.is_empty()) {
            format!("vs {opp}")
        } else {
            format!("{} vs {}", self.white, self.black)
        }
    }

    pub fn is_analyzed(&self) -> bool {
        self.has_analysis
            || self
                .analysis
                .as_ref()
                .map(|a| !a.moves.is_empty())
                .unwrap_or(false)
    }

    /// Analyzed and opened on the playbench for review.
    pub fn is_reviewed(&self) -> bool {
        self.reviewed
    }

    pub fn pivotal_moments(&self) -> Vec<&AnalyzedMove> {
        let Some(analysis) = &self.analysis else {
            return Vec::new();
        };
        let mut pivotal_moments: Vec<&AnalyzedMove> = analysis
            .moves
            .iter()
            .filter(|m| color_is_mine(&m.color, self.user_color.as_deref()))
            .filter(|m| {
                matches!(
                    m.classification.as_str(),
                    "inaccuracy" | "mistake" | "blunder"
                )
            })
            .collect();

        pivotal_moments.sort_by(|a, b| b.loss_cp.unwrap_or(0).cmp(&a.loss_cp.unwrap_or(0)));
        let mut top: Vec<&AnalyzedMove> = pivotal_moments.into_iter().take(5).collect();
        top.sort_by_key(|m| m.ply);
        top
    }

    pub fn outcome_for_user(&self) -> &'static str {
        let Some(result) = self.result.as_deref() else {
            return "—";
        };
        let color = self.user_color.as_deref().unwrap_or("white");
        match (result, color) {
            ("1-0", "white") | ("0-1", "black") => "won",
            ("0-1", "white") | ("1-0", "black") => "lost",
            ("1/2-1/2", _) => "drew",
            _ => "—",
        }
    }
}

fn color_is_mine(move_color: &str, user_color: Option<&str>) -> bool {
    let user_is_white = matches!(user_color.unwrap_or("white"), "white" | "w");
    let move_is_white = matches!(move_color, "white" | "w");
    user_is_white == move_is_white
}
