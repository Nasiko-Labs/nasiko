use synthetic_calculator::add;

#[test]
fn zero() { assert_eq!(add(0, 0), 0); }
#[test]
fn positive() { assert_eq!(add(2, 3), 5); }
#[test]
fn negative() { assert_eq!(add(-2, -3), -5); }
