use std::array;
use std::utils;


// ln 9 - 12
// TODO: check that these are good for word size 32
let RHO: int[] = [
    1,  3,  6,  10, 15, 21, 28, 36, 45, 55, 2,  14,
    27, 41, 56, 8,  25, 43, 62, 18, 39, 61, 20, 44
];

// ln 14 - 16
// TODO: check that these are good for word size 32
let PI: int[] = [
    10, 7,  11, 17, 18, 3, 5,  16, 8,  21, 24, 4,
    15, 23, 19, 13, 12, 2, 20, 14, 22, 9,  6,  1
];
// inverse of PI using 1 index plus zero at the beginning
// because rho pi step doesn't update st[0]
let PI_INVERSE: int[] = [
    0, 
    24, 18, 6, 12, 7, 23, 2, 9, 22, 1, 3, 17, 
    16, 20, 13, 8, 4, 5, 15, 19, 10, 21, 14, 11
];

// ln 19 - 28
// TODO: check that these are good for word size 32
let RC = std::array::map([
    0x0000000000000001, 0x0000000000008082, 0x800000000000808a,
    0x8000000080008000, 0x000000000000808b, 0x0000000080000001,
    0x8000000080008081, 0x8000000000008009, 0x000000000000008a,
    0x0000000000000088, 0x0000000080008009, 0x000000008000000a,
    0x000000008000808b, 0x800000000000008b, 0x8000000000008089,
    0x8000000000008003, 0x8000000000008002, 0x8000000000000080,
    0x000000000000800a, 0x800000008000000a, 0x8000000080008081,
    0x8000000000008080, 0x0000000080000001, 0x8000000080008008
], |x| Gate::Constant(x));

// Note that neither t nor bc is needed as they are both helper variables
// ln 52 - 55
let theta_bc = |s, st, i|
    std::array::fold([st[i + 5], st[i + 10], st[i + 15]], (s, st[i]), xor);

// ln 57 - 62
let theta_st = |s, st| map_enumerated_stateful(st, s, |s1, idx, elem| {
    let i = idx % 5;
    let j = idx / 5;
    let (s2, r) = rotl64(theta_bc(s1, st, (i + 1) % 5), 1);
    let t = xor(theta_bc(s2, st, (i + 4) % 5), r);
    xor(t, elem)
});

// ln 66 - 72
// rho pi
let rho_pi = |s, st, i| {
    let p = if i == 0 { 23 } else { i - 1 };
    rotl64((s, st[PI[p]]), RHO[i])
};

// TODO add the same helper for map_enumerated
let new_array_stateful = |l, s, f| std::utils::fold(l, |i| i, (s, []), |(s1, res), i| {
    let (s2, x) = f(s1, i);
    (s2, res + [x])
});

let map_enumerated_stateful = |arr, s, f|
    new_array_stateful(std::array::len(arr), s, |s1, i| f(s1, i, arr[i]));

// collect st_j
let rho_pi_loop = |(s, st)| new_array_stateful(25, s, |s1, i| if i == 0 { (s1, st[0]) } else { rho_pi(s1, st, i - 1) } );
// rearrange st_j
let rho_pi_rearrange = |(s, st)| (s, array::new(25, |i| st[PI_INVERSE[i]]));

// chi
let chi = |(s, st)| map_enumerated_stateful(st, s, |s1, idx, elem| {
    let i = idx / 5;
    let j = idx % 5;
    xor(and(not((s1, st[i * 5 + (j + 1) % 5])), st[i * 5 + (j + 2) % 5]), st[idx])
});

// ln 85 - 86
// iota
let iota: (int, Gate[]), int -> (int, Gate[]) = |(s, st), r| map_enumerated_stateful(st, s, |s1, idx, elem| if idx == 0 { xor((s1, elem), RC[r]) } else { (s1, elem) } ); // int[25], int -> int[25]

// ln 51 - 87
let r_loop = |(s, st)| {
    let (s_f, g) = utils::fold(24, |i| i, (s, st), |(s2, st2), r| iota(chi(rho_pi_rearrange(rho_pi_loop(theta_st(s2, st)))), r));
    let _ = std::debug::println("Gates:");
    let _ = std::debug::println(s_f);
    g
};

enum Gate {
    Input(int),
    Constant(int),
    Xor(Gate, Gate),
    And(Gate, Gate),
    Not(Gate),
    Rotl(Gate, int)
}

let input: int, int -> (int, Gate) = |s, i| (s + 1, Gate::Input(i));
let and = |(s, a), b| (s + 1, Gate::And(a, b));
let xor = |(s, a), b| (s + 1, Gate::Xor(a, b)); // TODO create ID
let not = |(s, a)| (s + 1, Gate::Not(a));
let rotl64 = |(s, a), n| (s + 1, Gate::Rotl(a, n));

let gate_count: Gate -> int = |g| match g {
    Gate::Input(_) => 1,
    Gate::Constant(_) => 1,
    Gate::Xor(a, b) => gate_count(a) + gate_count(b),
    Gate::And(a, b) => gate_count(a) + gate_count(b),
    Gate::Not(a) => gate_count(a) + 1,
    Gate::Rotl(a, _) => gate_count(a) + 1,
};

let gate_to_string: Gate -> string = |g| match g {
    Gate::Input(_) => "input",
    Gate::Constant(_) => "const",
    Gate::Xor(a, b) => "(" + gate_to_string(a) + " ^ " + gate_to_string(b) + ")",
    Gate::And(a, b) => "(" + gate_to_string(a) + " & " + gate_to_string(b) + ")",
    Gate::Not(a) => "~" + gate_to_string(a),
    Gate::Rotl(a, _) => "rotl(" + gate_to_string(a) + ")",
};


machine Main { 
    let x;

    let inputs = new_array_stateful(25, 0, |s, i| input(s, i));
    let circuit = r_loop(inputs);
    std::debug::println("Gate count:");
    std::debug::println(std::array::sum(std::array::map(circuit, |g| gate_count(g))));
}
