use crate::{
    config::{DEPTH, ZEROES},
    constant::{Data, EMPTYDATA, HASH_FUNCTION_64},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct IncrementalMerkleTree {
    pub filled_subtrees: [Data; DEPTH],
    pub root: Data,
    pub index: u32,
}

impl IncrementalMerkleTree {
    pub fn initial() -> Self {
        Self {
            filled_subtrees: [EMPTYDATA; DEPTH],
            root: ZEROES[DEPTH],
            index: 0,
        }
    }

    pub fn add(&mut self, a: Data) {
        let mut current_index = self.index;
        let mut current_level_hash = a;

        for i in 0..DEPTH {
            let (left, right) = if current_index % 2 == 0 {
                self.filled_subtrees[i] = current_level_hash;
                (current_level_hash, ZEROES[i])
            } else {
                (self.filled_subtrees[i], current_level_hash)
            };
            current_level_hash = HASH_FUNCTION_64(left, right);
            current_index /= 2;
        }
        self.root = current_level_hash;
        self.index += 1;
    }
}
