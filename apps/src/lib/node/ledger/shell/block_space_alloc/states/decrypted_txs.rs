use std::marker::PhantomData;

use super::super::{AllocFailure, BlockSpaceAllocator, TxBin};
use super::{
    BuildingDecryptedTxBatch, BuildingProtocolTxBatch, NextStateImpl, TryAlloc,
};

impl TryAlloc for BlockSpaceAllocator<BuildingDecryptedTxBatch> {
    #[inline]
    fn try_alloc(&mut self, tx: &[u8]) -> Result<(), AllocFailure> {
        self.decrypted_txs.try_dump(tx)
    }
}

impl NextStateImpl for BlockSpaceAllocator<BuildingDecryptedTxBatch> {
    type Next = BlockSpaceAllocator<BuildingProtocolTxBatch>;

    #[inline]
    fn next_state_impl(mut self) -> Self::Next {
        self.decrypted_txs.shrink_to_fit();

        // the remaining space is allocated to protocol txs
        let remaining_free_space = self.uninitialized_space_in_bytes();
        self.protocol_txs = TxBin::init(remaining_free_space);

        // cast state
        let Self {
            block,
            protocol_txs,
            encrypted_txs,
            decrypted_txs,
            ..
        } = self;

        BlockSpaceAllocator {
            _state: PhantomData,
            block,
            protocol_txs,
            encrypted_txs,
            decrypted_txs,
        }
    }
}
