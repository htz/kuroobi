//! What the solver orders moves and seeds its windows with.

#[cfg(not(feature = "nnue-order"))]
pub use crate::linear::Linear as OrderEval;

#[cfg(feature = "nnue-order")]
pub use nnue_order::OrderEval;

#[cfg(feature = "nnue-order")]
mod nnue_order {
    use crate::board::Board;
    use crate::color::Color;
    use crate::nnue::Nnue;
    use crate::pattern_index::{PatternIndexer, PatternIndices};
    use std::sync::Arc;

    /// The NNUE standing in for the linear evaluator, for builds that ship one model.
    pub struct OrderEval(pub Arc<Nnue>);

    impl OrderEval {
        pub fn indexer(&self) -> &PatternIndexer {
            self.0.indexer()
        }

        pub fn eval_indices(&self, board: &Board, indices: &PatternIndices) -> f32 {
            self.0.eval_from_indices(indices, board)
        }

        pub fn eval_order_bb(
            &self,
            player: u64,
            opponent: u64,
            color: Color,
            indices: &PatternIndices,
        ) -> f32 {
            let black = matches!(color, Color::Black);
            let mut b = Board::new();
            b.black = if black { player } else { opponent };
            b.white = if black { opponent } else { player };
            b.player = color;
            b.empty_count = 64 - (player | opponent).count_ones() as u8;
            self.0.eval_from_indices(indices, &b)
        }
    }
}
