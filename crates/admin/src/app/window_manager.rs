use std::ops::{Deref, DerefMut};
use std::slice;

/// A thin wrapper around `Vec<T>` for window-type collections.
///
/// Provides convenience methods common to all window managers
/// while remaining fully compatible with existing APIs that take `&mut Vec<T>`.
#[derive(Default)]
pub struct WindowVec<T> {
    inner: Vec<T>,
}

// ── construction ──────────────────────────────────────────────

impl<T> WindowVec<T> {
    pub fn new() -> Self {
        Self { inner: Vec::new() }
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            inner: Vec::with_capacity(cap),
        }
    }
}

// ── Vec pass-through methods ─────────────────────────────────
//
// These exist so that the borrow checker can see that only the
// `inner` field is touched, avoiding the "cannot borrow `*self`
// as immutable because it is also borrowed as mutable" error that
// occurs when the same operation is performed through `DerefMut`.

impl<T> WindowVec<T> {
    /// Push a value onto the back.
    #[inline]
    pub fn push(&mut self, value: T) {
        self.inner.push(value);
    }

    /// Remove the last value (if any).
    #[inline]
    pub fn pop(&mut self) -> Option<T> {
        self.inner.pop()
    }

    /// Returns `true` if the vector contains no elements.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns the number of elements.
    #[inline]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Remove the element at `index`.
    #[inline]
    pub fn remove(&mut self, index: usize) -> T {
        self.inner.remove(index)
    }

    /// Retain only elements satisfying the predicate.
    #[inline]
    pub fn retain<F>(&mut self, f: F)
    where
        F: FnMut(&T) -> bool,
    {
        self.inner.retain(f);
    }
}

// ── iteration ─────────────────────────────────────────────────

impl<T> WindowVec<T> {
    /// Iterate over shared references.
    pub fn iter(&self) -> slice::Iter<'_, T> {
        self.inner.iter()
    }

    /// Iterate over mutable references.  Returns a concrete type
    /// that supports `.rev()`, `.enumerate()`, etc.
    pub fn iter_mut(&mut self) -> slice::IterMut<'_, T> {
        self.inner.iter_mut()
    }
}

// `for window in &self.command_windows { .. }`
impl<'a, T> IntoIterator for &'a WindowVec<T> {
    type Item = &'a T;
    type IntoIter = slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.iter()
    }
}

// `for window in &mut self.command_windows { .. }`
impl<'a, T> IntoIterator for &'a mut WindowVec<T> {
    type Item = &'a mut T;
    type IntoIter = slice::IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.iter_mut()
    }
}

// ── convenience ───────────────────────────────────────────────

impl<T> WindowVec<T> {
    /// Number of open windows.
    pub fn count(&self) -> usize {
        self.inner.len()
    }

    /// Whether any window is open.
    pub fn is_open(&self) -> bool {
        !self.inner.is_empty()
    }

    /// Add a window. Silently ignores if the window already exists
    /// (checked via `Fn(&T) -> bool`).
    pub fn add_unique(&mut self, window: T, exists: impl Fn(&T) -> bool) {
        if !self.inner.iter().any(exists) {
            self.inner.push(window);
        }
    }
}

// ── compatibility with existing APIs expecting `&mut Vec<T>` ──

impl<T> Deref for WindowVec<T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Vec<T> {
        &self.inner
    }
}

impl<T> DerefMut for WindowVec<T> {
    fn deref_mut(&mut self) -> &mut Vec<T> {
        &mut self.inner
    }
}
