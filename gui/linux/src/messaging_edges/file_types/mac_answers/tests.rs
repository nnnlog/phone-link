use std::collections::HashSet;

use super::*;

#[test]
fn the_tables_name_lowercase_types_once() {
    let names: Vec<&str> = MAC_ANSWERS
        .iter()
        .map(|(name, _)| *name)
        .chain(MAC_ALIASES.iter().map(|(alias, _)| *alias))
        .chain(DOCUMENT_TYPES)
        .collect();
    for name in &names {
        assert_eq!(*name, name.to_lowercase());
    }
    let answered: HashSet<&str> = MAC_ANSWERS.iter().map(|(name, _)| *name).collect();
    assert_eq!(answered.len(), MAC_ANSWERS.len());
    assert!(MAC_ALIASES.iter().all(|(alias, _)| !answered.contains(alias)), "an alias is answered as its type");
    assert_eq!(DOCUMENT_TYPES.iter().collect::<HashSet<_>>().len(), DOCUMENT_TYPES.len());
}
