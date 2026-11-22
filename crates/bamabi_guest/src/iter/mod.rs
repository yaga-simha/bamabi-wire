//! Safe chunk iterators, dense column slices, and indirect sparse-set row views.

pub mod chunk;
pub mod chunk_iter;
pub mod filters;
pub mod slice;
pub mod typed_query;

pub use bamabi_guest_sys::{DrainOptions, SparseRows, SparseRowsIter, SparseRowsMut};
pub use chunk::Chunk;
pub use chunk_iter::ChunkIter;
pub use slice::{ChunkSlice, ChunkSliceMut};
pub use typed_query::{
    Added, AddedMut, Allow, Changed, ChangedMut, CopyableQueryTerm, CopyableQueryTuple, Has, Mut,
    MutSlice, Or, Or2, Or3, OrFilterTerm, OrFilterTuple, PairwiseQueryTerm, PairwiseQueryTuple,
    QuerySingleError, QueryTerm, QueryTuple, Ref, RefSlice, TypedChunkIter, With, WithAdded,
    WithChanged, Without,
};
