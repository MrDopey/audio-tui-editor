//! Wraparound index search shared by BROWSE's file search and METADATA's
//! field search (design §5, §18): the three ways a `/` search is walked,
//! independent of what's being searched.

/// Live "as you type" preview: search forward starting at (and including)
/// `origin`, wrapping once around the full range. Used so the entry under
/// the cursor previews before the search is confirmed.
pub(super) fn find_from(origin: usize, count: usize, pred: impl Fn(usize) -> bool) -> Option<usize> {
    (0..count).map(|offset| (origin + offset) % count).find(|&i| pred(i))
}

/// `n`: repeat the last search forward, starting just after `origin` and
/// wrapping all the way back around to `origin` itself — so a single match
/// still "finds" itself.
pub(super) fn find_forward(origin: usize, count: usize, pred: impl Fn(usize) -> bool) -> Option<usize> {
    (1..=count)
        .map(|offset| (origin + offset) % count)
        .find(|&i| pred(i))
}

/// `N`: repeat the last search backward, same wraparound as [`find_forward`].
pub(super) fn find_backward(origin: usize, count: usize, pred: impl Fn(usize) -> bool) -> Option<usize> {
    (1..=count)
        .map(|offset| (origin + count * count - offset) % count)
        .find(|&i| pred(i))
}
