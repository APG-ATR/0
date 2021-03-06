use super::*;
use swc_ecma_parser::Syntax;

fn syntax() -> Syntax {
    ::swc_ecma_parser::Syntax::default()
}

fn tr(_: ()) -> impl Pass {
    ComputedProps
}

test!(
    ::swc_ecma_parser::Syntax::default(),
    |_| ComputedProps,
    issue_210,
    "
const b = {[a]: 1}
export const c = {[a]: 1}
",
    "const b = _defineProperty({
}, a, 1);
export const c = _defineProperty({
}, a, 1);"
);

test!(
    ::swc_ecma_parser::Syntax::default(),
    |_| ComputedProps,
    accessors,
    r#"var obj = {
  get [foobar]() {
    return "foobar";
  },
  set [foobar](x) {
    console.log(x);
  },
  get test() {
    return "regular getter after computed property";
  },
  set "test"(x) {
    console.log(x);
  }
};
"#,
    r#"var _obj, _mutatorMap = {
};
var obj = ( _obj = {
}, _mutatorMap[foobar] = _mutatorMap[foobar] || {
}, _mutatorMap[foobar].get = function() {
    return 'foobar';
}, _mutatorMap[foobar] = _mutatorMap[foobar] || {
}, _mutatorMap[foobar].set = function(x) {
    console.log(x);
}, _mutatorMap['test'] = _mutatorMap['test'] || {
}, _mutatorMap['test'].get = function() {
    return 'regular getter after computed property';
}, _mutatorMap['test'] = _mutatorMap['test'] || {
}, _mutatorMap['test'].set = function(x) {
    console.log(x);
}, _defineEnumerableProperties(_obj, _mutatorMap), _obj);"#
);

test!(
    ::swc_ecma_parser::Syntax::default(),
    |_| ComputedProps,
    argument,
    r#"foo({
  [bar]: "foobar"
});"#,
    r#"foo(_defineProperty({}, bar, "foobar"));
"#
);

test!(
    ::swc_ecma_parser::Syntax::default(),
    |_| ComputedProps,
    assignment,
    r#"foo = {
  [bar]: "foobar"
};"#,
    r#"foo = _defineProperty({}, bar, "foobar");"#
);

test!(
    ::swc_ecma_parser::Syntax::default(),
    |_| ComputedProps,
    method,
    r#"var obj = {
  [foobar]() {
    return "foobar";
  },
  test() {
    return "regular method after computed property";
  }
};"#,
    r#"var _obj;

var obj = (_obj = {}, _defineProperty(_obj, foobar, function () {
  return "foobar";
}), _defineProperty(_obj, "test", function () {
  return "regular method after computed property";
}), _obj);"#
);

test!(
    ::swc_ecma_parser::Syntax::default(),
    |_| ComputedProps,
    mixed,
    r#"var obj = {
  ["x" + foo]: "heh",
  ["y" + bar]: "noo",
  foo: "foo",
  bar: "bar"
};"#,
    r#"var _obj;

var obj = (_obj = {}, _defineProperty(_obj, "x" + foo, "heh"), _defineProperty(_obj,
 "y" + bar, "noo"), _defineProperty(_obj, "foo", "foo"), _defineProperty(_obj, "bar", "bar"), _obj);"#
);

test!(
    ::swc_ecma_parser::Syntax::default(),
    |_| ComputedProps,
    multiple,
    r#"var obj = {
  ["x" + foo]: "heh",
  ["y" + bar]: "noo"
};"#,
    r#"var _obj;

var obj = (_obj = {}, _defineProperty(_obj, "x" + foo, "heh"), 
_defineProperty(_obj, "y" + bar, "noo"), _obj);"#
);

test!(
    ::swc_ecma_parser::Syntax::default(),
    |_| ComputedProps,
    single,
    r#"var obj = {
  ["x" + foo]: "heh"
};"#,
    r#"var obj = _defineProperty({}, "x" + foo, "heh");"#
);

test!(
    ::swc_ecma_parser::Syntax::default(),
    |_| ComputedProps,
    symbol,
    r#"var k = Symbol();
var foo = {
  [Symbol.iterator]: "foobar",
  get [k]() {
    return k;
  }
};
"#,
    r#"var k = Symbol();
var _obj, _mutatorMap = {
};
var foo = ( _obj = {
}, _defineProperty(_obj, Symbol.iterator, 'foobar'), _mutatorMap[k] = _mutatorMap[k] || {
}, _mutatorMap[k].get = function() {
    return k;
}, _defineEnumerableProperties(_obj, _mutatorMap), _obj);"#
);

test_exec!(
    ::swc_ecma_parser::Syntax::default(),
    |_| ComputedProps,
    symbol_exec,
    r#"
var k = Symbol();
var foo = {
  [Symbol.iterator]: "foobar",
  get [k]() {
    return k;
  }
};

expect(foo[Symbol.iterator]).toBe("foobar")
expect(foo[k]).toBe(k)"#
);

test!(
    ::swc_ecma_parser::Syntax::default(),
    |_| ComputedProps,
    this,
    r#"var obj = {
  ["x" + foo.bar]: "heh"
};"#,
    r#"var obj = _defineProperty({}, "x" + foo.bar, "heh");"#
);

test!(
    ::swc_ecma_parser::Syntax::default(),
    |_| ComputedProps,
    issue_315_1,
    "
({
  foo: {
    bar: null,
    [baz]: null
  },

  [bon]: {
    flab: null
  }
});

export function corge() {}
",
    "
_defineProperty({
    foo: _defineProperty({
        bar: null
    }, baz, null)
}, bon, {
    flab: null
});
export function corge() {
}"
);

test!(
    ::swc_ecma_parser::Syntax::default(),
    |_| ComputedProps,
    issue_315_2,
    "
export function corge() {}
",
    "
export function corge() {}
"
);

test!(
    ::swc_ecma_parser::Syntax::default(),
    |_| ComputedProps,
    issue_315_3,
    "
export function corge() {}

({
  foo: {
    bar: null,
    [baz]: null
  },

  [bon]: {
    flab: null
  }
});
",
    "
export function corge() {
}
_defineProperty({
    foo: _defineProperty({
        bar: null
    }, baz, null)
}, bon, {
    flab: null
});
"
);

test!(
    ::swc_ecma_parser::Syntax::default(),
    |_| ComputedProps,
    issue_315_4,
    "
export class Foo {}

({
  foo: {
    bar: null,
    [baz]: null
  },

  [bon]: {
    flab: null
  }
});
",
    "
export class Foo {}

_defineProperty({
    foo: _defineProperty({
        bar: null
    }, baz, null)
}, bon, {
    flab: null
});
"
);

// spec_mixed
test!(
    syntax(),
    |_| tr(Default::default()),
    spec_mixed,
    r#"
var obj = {
  ["x" + foo]: "heh",
  ["y" + bar]: "noo",
  foo: "foo",
  bar: "bar"
};

"#,
    r#"
var _obj;

var obj = (_obj = {}, _defineProperty(_obj, "x" + foo, "heh"), _defineProperty(_obj, "y" + bar, "noo"), _defineProperty(_obj, "foo", "foo"), _defineProperty(_obj, "bar", "bar"), _obj);

"#
);

// spec_single
test!(
    syntax(),
    |_| tr(Default::default()),
    spec_single,
    r#"
var obj = {
  ["x" + foo]: "heh"
};

"#,
    r#"
var obj = _defineProperty({}, "x" + foo, "heh");

"#
);

// spec

// regression_7144
test!(
    syntax(),
    |_| tr(Default::default()),
    regression_7144,
    r#"
export default {
  [a]: b,
  [c]: d
};
"#,
    r#"
var _obj;

export default (_obj = {}, _defineProperty(_obj, a, b), _defineProperty(_obj, c, d), _obj);

"#
);

// spec_multiple
test!(
    syntax(),
    |_| tr(Default::default()),
    spec_multiple,
    r#"
var obj = {
  ["x" + foo]: "heh",
  ["y" + bar]: "noo"
};

"#,
    r#"
var _obj;

var obj = (_obj = {}, _defineProperty(_obj, "x" + foo, "heh"), _defineProperty(_obj, "y" + bar, "noo"), _obj);

"#
);

// spec_this
test!(
    syntax(),
    |_| tr(Default::default()),
    spec_this,
    r#"
var obj = {
  ["x" + foo.bar]: "heh"
};

"#,
    r#"
var obj = _defineProperty({}, "x" + foo.bar, "heh");

"#
);

// spec_method
test!(
    syntax(),
    |_| tr(Default::default()),
    spec_method,
    r#"
var obj = {
  [foobar]() {
    return "foobar";
  },
  test() {
    return "regular method after computed property";
  }
};

"#,
    r#"
var _obj;

var obj = (_obj = {}, _defineProperty(_obj, foobar, function () {
  return "foobar";
}), _defineProperty(_obj, "test", function () {
  return "regular method after computed property";
}), _obj);

"#
);

// spec_assignment
test!(
    syntax(),
    |_| tr(Default::default()),
    spec_assignment,
    r#"
foo = {
  [bar]: "foobar"
};

"#,
    r#"
foo = _defineProperty({}, bar, "foobar");

"#
);

// spec_argument
test!(
    syntax(),
    |_| tr(Default::default()),
    spec_argument,
    r#"
foo({
  [bar]: "foobar"
});

"#,
    r#"
foo(_defineProperty({}, bar, "foobar"));

"#
);

// spec_two
test!(
    syntax(),
    |_| tr(Default::default()),
    spec_two,
    r#"
var obj = {
  first: "first",
  ["second"]: "second",
};

"#,
    r#"
var obj = _defineProperty({
  first: "first"
}, "second", "second");

"#
);

// spec_variable
test!(
    syntax(),
    |_| tr(Default::default()),
    spec_variable,
    r#"
var foo = {
  [bar]: "foobar"
};

"#,
    r#"
var foo = _defineProperty({}, bar, "foobar");

"#
);

// spec_symbol_exec
test_exec!(
    syntax(),
    |_| tr(Default::default()),
    spec_symbol_exec,
    r#"
var k = Symbol();
var foo = {
  [Symbol.iterator]: "foobar",
  get [k]() {
    return k;
  }
};

expect(foo[Symbol.iterator]).toBe("foobar")
expect(foo[k]).toBe(k)

"#
);
