const { WasmCompiler } = require('../pkg/nwscript_compiler.js');

let pass = 0, fail = 0;
const failures = [];

function expectError(name, src, errorSubstring) {
    const c = new WasmCompiler();
    c.setRequireEntryPoint(false);
    c.setCollectAllErrors(true);
    c.addFile('t', src);
    c.compile('t');
    const n = c.getCollectedErrorCount();
    const errors = [];
    for (let i = 0; i < n; i++) errors.push(c.getCollectedError(i));
    c.free();

    const got = errors.join(' | ').toLowerCase();
    if (n > 0 && (!errorSubstring || got.includes(errorSubstring.toLowerCase()))) {
        pass++;
        console.log(`✓ ${name}`);
    } else {
        fail++;
        failures.push({ name, errors, expected: errorSubstring });
        console.log(`✗ ${name} — expected error containing "${errorSubstring}", got: ${n} errors: ${errors[0] || 'NONE'}`);
    }
}

function expectOk(name, src) {
    const c = new WasmCompiler();
    c.setRequireEntryPoint(false);
    c.setCollectAllErrors(true);
    c.addFile('t', src);
    c.compile('t');
    const n = c.getCollectedErrorCount();
    const errors = [];
    for (let i = 0; i < n; i++) errors.push(c.getCollectedError(i));
    c.free();

    if (n === 0) {
        pass++;
        console.log(`✓ ${name}`);
    } else {
        fail++;
        failures.push({ name, errors });
        console.log(`✗ ${name} — expected no errors, got: ${errors[0]}`);
    }
}

console.log('=== Type mismatch ===');
expectError('global string = int', 'string X = 1;', 'mismatched');
expectError('global int = string', 'int X = "hello";', 'mismatched');
expectError('global float = string', 'float X = "hello";', 'mismatched');
expectError('global object = int', 'object X = 1;', 'mismatched');
expectError('const string = int', 'const string X = 1;', 'mismatched');
expectError('const int = string', 'const int X = "hello";', 'mismatched');
expectError('local string = int', 'void main() { string s = 1; }', 'mismatched');
expectError('local int = string', 'void main() { int x = "hi"; }', 'mismatched');
expectError('assign string to int var', 'void main() { int x = 0; x = "hi"; }', 'mismatched');
expectError('return int from string func', 'string foo() { return 1; }', 'mismatched');
expectError('return value in void func', 'void foo() { return 1; }', 'mismatched');

expectOk('global int = int', 'int X = 1;');
expectOk('global float = float', 'float X = 1.5;');
expectOk('global string = string', 'string X = "hi";');
expectOk('local int = int', 'void main() { int x = 5; }');

console.log('\n=== Function call errors ===');
expectError('undefined function', 'void main() { undefinedFn(); }', 'undefined identifier');
expectError('wrong arg type', 'void foo(int a) { }\nvoid main() { foo("string"); }', 'mismatched');
expectError('too few args (no defaults)', 'void foo(int a, int b) { }\nvoid main() { foo(1); }', 'does not match');
expectError('too many args', 'void foo(int a) { }\nvoid main() { foo(1, 2); }', 'does not match');

expectOk('correct call', 'void foo(int a) { }\nvoid main() { foo(1); }');
expectOk('default param skipped', 'void foo(int a, int b = 10) { }\nvoid main() { foo(1); }');

console.log('\n=== Control flow errors ===');
expectError('break outside loop', 'void main() { break; }', 'break outside');
expectError('continue outside loop', 'void main() { continue; }', 'break outside');
expectError('switch with non-integer', 'void main() { string s = "x"; switch (s) { default: break; } }', 'switch expression');
expectError('duplicate case', 'void main() { switch (1) { case 1: break; case 1: break; } }', 'multiple case');
expectError('duplicate default', 'void main() { switch (1) { default: break; default: break; } }', 'multiple default');
expectError('non-const case', 'void main() { int v = 5; switch (1) { case v + 1: break; } }', 'case parameter');
expectError('not all paths return', 'int foo() { }', 'control paths');

expectOk('break in for', 'void main() { for (int i=0; i<10; i++) { break; } }');
expectOk('break in switch', 'void main() { switch (1) { case 1: break; } }');
expectOk('all paths return', 'int foo(int x) { if (x>0) return 1; else return 0; }');

console.log('\n=== Operator errors ===');
expectError('add int + string', 'void main() { int x = 1 + "hi"; }', 'arithmetic');
expectError('subtract strings', 'void main() { string s = "a" - "b"; }', 'mismatched');

expectOk('add int + int', 'void main() { int x = 1 + 2; }');
expectOk('concat string + string', 'void main() { string s = "a" + "b"; }');
expectOk('add float + int promotes', 'void main() { float f = 1.0 + 2; }');

console.log('\n=== Function decl errors ===');
expectError('non-optional after optional', 'void foo(int a = 5, int b);', 'optional');
expectError('duplicate impl', 'void foo() { }\nvoid foo() { }', 'duplicate');
expectError('mismatched signatures', 'int foo();\nfloat foo() { return 1.0; }', 'differ');

expectOk('decl then impl matching', 'void foo();\nvoid foo() { }');
expectOk('optional params', 'void foo(int a, int b = 10, string c = "x") { }');

console.log('\n=== Syntax errors ===');
expectError('missing semicolon', 'int x = 5\nint y = 6;', 'unexpected');
expectError('incomplete declaration', 'int x = ;', null);
expectError('unclosed brace', 'void main() {', null);
expectError('unclosed paren', 'void main() { int x = (1 + 2; }', null);

console.log('\n=== Struct errors ===');
expectError('undefined struct field', 'struct P { int a; };\nvoid main() { struct P p; int x = p.b; }', 'undefined field');

expectOk('struct field access', 'struct P { int a; };\nvoid main() { struct P p; int x = p.a; }');
expectOk('nested struct', 'struct A { int x; };\nstruct B { struct A a; };\nvoid main() { struct B b; int x = b.a.x; }');

console.log('\n=== Include errors ===');
expectError('include not found', '#include "nonexistent"\nvoid main() { }', 'file not found');

console.log('\n=== Variable scope ===');
expectError('redeclare in same scope', 'void main() { int x = 1; int x = 2; }', 'already used');
expectError('use before declare', 'void main() { int x = y; int y = 5; }', 'undefined identifier');
expectError('out of scope', 'void main() { if (1) { int x = 5; } int y = x; }', 'undefined identifier');

expectOk('redeclare in nested scope', 'void main() { int x = 1; if (1) { int x = 2; } }');

console.log(`\n=== ${pass + fail} tests: ${pass} passed, ${fail} failed ===`);
if (fail > 0) {
    console.log('\nFailures:');
    failures.forEach(f => {
        console.log(`  ${f.name}`);
        if (f.expected) console.log(`    expected: "${f.expected}"`);
        if (f.errors.length) f.errors.forEach(e => console.log(`    got: ${e}`));
        else console.log('    got: NO errors');
    });
}
process.exit(fail > 0 ? 1 : 0);
