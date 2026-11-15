//! Sorting a table by one of its columns, either way.

use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortOrder {
    Ascending,
    Descending,
}

impl SortOrder {
    #[must_use]
    pub fn flipped(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }

    /// How the panel's preferences save it.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Ascending => "asc",
            Self::Descending => "desc",
        }
    }

    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "asc" => Some(Self::Ascending),
            "desc" => Some(Self::Descending),
            _ => None,
        }
    }

    /// `ordering` as this order puts it: ascending keeps it, descending turns it round.
    #[must_use]
    pub fn apply(self, ordering: Ordering) -> Ordering {
        match self {
            Self::Ascending => ordering,
            Self::Descending => ordering.reverse(),
        }
    }
}

/// A table's column, which knows the order it sorts in first: the one most often wanted, such
/// as the worst connections at the top.
pub trait Column: Copy + Eq + 'static {
    /// Every column, to find one by its key.
    const ALL: &'static [Self];

    fn first_order(self) -> SortOrder;

    /// How the panel's preferences save it.
    fn key(self) -> &'static str;
}

/// Which column a table is sorted by, and which way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort<C> {
    pub column: C,
    pub order: SortOrder,
}

impl<C: Column> Sort<C> {
    #[must_use]
    pub fn by(column: C) -> Self {
        Self {
            column,
            order: column.first_order(),
        }
    }

    /// How the panel's preferences save it, as `loss:desc`.
    #[must_use]
    pub fn key(self) -> String {
        format!("{}:{}", self.column.key(), self.order.key())
    }

    /// Reads what `key` writes; None for anything else.
    #[must_use]
    pub fn from_key(text: &str) -> Option<Self> {
        let (column, order) = text.split_once(':')?;
        Some(Self {
            column: C::ALL.iter().copied().find(|known| known.key() == column)?,
            order: SortOrder::from_key(order)?,
        })
    }

    /// The sort after selecting `column`'s heading: the other way if it is already the one,
    /// otherwise by it in its first order.
    #[must_use]
    pub fn select(self, column: C) -> Self {
        if column == self.column {
            Self {
                column,
                order: self.order.flipped(),
            }
        } else {
            Self::by(column)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Column, Sort, SortOrder};
    use std::cmp::Ordering;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Test {
        Name,
        Worst,
    }

    impl Column for Test {
        const ALL: &'static [Self] = &[Self::Name, Self::Worst];

        fn first_order(self) -> SortOrder {
            match self {
                Self::Name => SortOrder::Ascending,
                Self::Worst => SortOrder::Descending,
            }
        }

        fn key(self) -> &'static str {
            match self {
                Self::Name => "name",
                Self::Worst => "worst",
            }
        }
    }

    #[test]
    fn sorts_read_back_and_anything_odd_is_refused() {
        let sort = Sort::by(Test::Worst).select(Test::Worst);
        assert_eq!(sort.key(), "worst:asc");
        assert_eq!(Sort::from_key("worst:asc"), Some(sort));
        assert_eq!(Sort::<Test>::from_key("worst"), None);
        assert_eq!(Sort::<Test>::from_key("best:asc"), None);
        assert_eq!(SortOrder::from_key("desc"), Some(SortOrder::Descending));
        assert_eq!(SortOrder::from_key("down"), None);
    }

    #[test]
    fn selecting_a_column_sorts_by_it_and_again_turns_it_round() {
        let sort = Sort::by(Test::Name);
        assert_eq!(sort.order, SortOrder::Ascending);
        let sort = sort.select(Test::Worst);
        assert_eq!(
            (sort.column, sort.order),
            (Test::Worst, SortOrder::Descending)
        );
        let sort = sort.select(Test::Worst);
        assert_eq!(sort.order, SortOrder::Ascending);
        assert_eq!(
            SortOrder::Descending.apply(Ordering::Less),
            Ordering::Greater
        );
    }
}
