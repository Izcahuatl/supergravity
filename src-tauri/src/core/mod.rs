pub mod providers;

pub mod agent;
pub mod approvals;
pub mod error;
pub mod store;
pub mod tools;
pub mod types;

#[cfg(test)]
mod tests {
    #[test]
    fn scaffold_smoke() {
        assert_eq!(2 + 2, 4);
    }
}
