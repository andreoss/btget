use bt::random::Source;

#[test]
fn successive_values_are_not_a_counter() {
    let source = Source::new();
    let values: Vec<u64> = (0..8).map(|_| source.next_u64()).collect();
    assert!(
        values.windows(2).all(|w| w[0] != w[1]),
        "repeated value: {:?}",
        values
    );
    assert!(
        values.windows(2).any(|w| w[1] != w[0].wrapping_add(1)),
        "values increment like a counter: {:?}",
        values
    );
}

#[test]
fn separate_sources_differ() {
    let first = Source::new();
    let second = Source::new();
    assert_ne!(first.next_u64(), second.next_u64());
}

#[test]
fn fill_covers_every_byte() {
    let source = Source::new();
    let mut a = [0u8; 20];
    let mut b = [0u8; 20];
    source.fill(&mut a);
    source.fill(&mut b);
    assert_ne!(a, b);
    assert!(a.iter().any(|byte| *byte != 0));
    let mut long = [0u8; 48];
    source.fill(&mut long);
    assert!(long[40..].iter().any(|byte| *byte != 0));
}
