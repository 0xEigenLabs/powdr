use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};

type Clause = Vec<i64>;

fn main() {
    let clauses = read_dimacs_file(&std::env::args().nth(1).unwrap());
    let lrat_proof = read_lrat_file(&std::env::args().nth(2).unwrap());
    check_lrat(clauses, lrat_proof);
}

fn read_dimacs_file(path: &str) -> Vec<Clause> {
    // println!("{path}");
    let data = fs::read_to_string(path).unwrap();
    let mut items = data.split_whitespace();

    assert_eq!(items.next(), Some("p"));
    assert_eq!(items.next(), Some("cnf"));
    items.next();
    items.next();

    let mut items = items.map(|s| s.parse::<i64>().unwrap());

    let mut clauses = vec![];
    while let Some(clause) = read_dimacs_clause(&mut items) {
        clauses.push(clause);
    }
    clauses
}

fn read_dimacs_clause(iter: &mut impl Iterator<Item = i64>) -> Option<Clause> {
    let mut clause = vec![];
    loop {
        let lit = iter.next()?;
        if lit == 0 {
            break;
        }
        clause.push(lit)
    }
    Some(clause)
}

#[derive(Debug)]
struct LratItem {
    id: u64,
    clause: Clause,
    /// Clauses to run unit propagation on.
    direct_hints: Vec<u64>,
    /// RAT checks and following clauses for unit propagation.
    rat_hints: Vec<(u64, Vec<u64>)>,
}

fn read_lrat_file(path: &str) -> Vec<LratItem> {
    let data = fs::read_to_string(path).unwrap();
    let mut items = data.split_whitespace();

    let mut rats = vec![];
    while let Some(item) = read_rat_item(&mut items) {
        rats.push(item);
    }
    rats
}

fn read_rat_item<'a>(iter: &mut impl Iterator<Item = &'a str>) -> Option<LratItem> {
    let id = iter.next()?.parse::<u64>().unwrap();

    let token = iter.next()?;
    if token == "d" {
        while iter.next()?.parse::<u64>().unwrap() != 0 {}
        read_rat_item(iter)
    } else {
        let mut clause = vec![];
        let mut lit = token.parse::<i64>().unwrap();
        while lit != 0 {
            clause.push(lit);
            lit = iter.next()?.parse::<i64>().unwrap();
        }

        let (direct_hints, mut rat_hint_clause) = read_positive_numbers(iter)?;
        let mut rat_hints = vec![];
        while rat_hint_clause != 0 {
            let (subsequent, next_rat_hint_clause) = read_positive_numbers(iter)?;
            rat_hints.push((rat_hint_clause.unsigned_abs(), subsequent));
            rat_hint_clause = next_rat_hint_clause;
        }
        Some(LratItem {
            id,
            clause,
            direct_hints,
            rat_hints,
        })
    }
}

/// Reads a sequence of positive numbers followed by a zero or a negative number
fn read_positive_numbers<'a>(iter: &mut impl Iterator<Item = &'a str>) -> Option<(Vec<u64>, i64)> {
    let mut numbers = vec![];
    loop {
        let n = iter.next()?.parse::<i64>().unwrap();
        if n <= 0 {
            return Some((numbers, n));
        }
        numbers.push(n as u64)
    }
}

fn check_lrat(clauses: Vec<Clause>, rats: Vec<LratItem>) {
    let mut clauses = clauses
        .into_iter()
        .enumerate()
        .map(|(i, c)| ((i + 1) as u64, c))
        .collect::<BTreeMap<_, _>>();
    assert!(rats.last().unwrap().clause.is_empty());
    for rat in rats {
        verify_rat(&clauses, &rat);
        assert!(clauses.insert(rat.id, rat.clause).is_none());
    }
}

fn verify_rat(clauses: &BTreeMap<u64, Clause>, rat: &LratItem) {
    // println!("Verifying {:?}", rat.clause);
    let pivot = rat.clause.first().cloned();
    let assignments = rat.clause.iter().cloned().collect::<BTreeSet<_>>();
    // println!("Running direct unit propagation");
    let (derived_empty_clause, assignments) =
        unit_propagate(clauses, assignments, &rat.direct_hints);
    if derived_empty_clause {
        assert!(rat.rat_hints.is_empty());
        return;
    }

    // println!("Running RAT checks");
    // TOOD we still have to check that the rat_hints
    // are all clauses that have the pivot in the other polarity.
    // Does that include newly generated clauses?
    // If not, an efficient way could be to create a literal->clause mapping.
    // Or we just scan through the whole cnf...
    for (rat_candidate, propagators) in &rat.rat_hints {
        // println!("Rat candidate: {:?}", clauses[rat_candidate]);
        let mut assignments = assignments.clone();
        let pivot = pivot.unwrap();
        let mut rat_cadidate_literals = clauses[rat_candidate].iter().filter(|lit| **lit != pivot);
        if propagators.is_empty() {
            assert!(rat_cadidate_literals.any(|lit| assignments.contains(&-lit)));
        } else {
            assignments.extend(rat_cadidate_literals.cloned());
            assert!(unit_propagate(clauses, assignments, propagators).0);
        }
    }
    // println!("Done");
}

fn unit_propagate(
    clauses: &BTreeMap<u64, Clause>,
    mut assignments: BTreeSet<i64>,
    hints: &[u64],
) -> (bool, BTreeSet<i64>) {
    let mut derived_empty_clause = false;
    for hint in hints.iter().map(|h| &clauses[h]) {
        // println!("Propagating {hint:?}");
        assert!(!derived_empty_clause);
        let mut remaining = hint.iter().filter(|lit| !assignments.contains(lit));
        if let Some(lit) = remaining.next() {
            assert!(remaining.next().is_none());
            // println!("Assigned: {}", -lit);
            assert!(!assignments.contains(lit));
            assignments.insert(-lit);
        } else {
            derived_empty_clause = true;
            // println!("Empty clause");
        }
    }
    (derived_empty_clause, assignments)
}