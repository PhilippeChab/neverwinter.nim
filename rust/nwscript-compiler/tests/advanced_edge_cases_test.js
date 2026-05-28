const { WasmCompiler } = require('../pkg/nwscript_compiler.js');

let pass = 0, fail = 0;
const failures = [];

function check(name, expectError, src, substr) {
    const c = new WasmCompiler();
    c.setRequireEntryPoint(false);
    c.setCollectAllErrors(true);
    c.addFile('t', src);
    c.compile('t');
    const errs = [];
    for (let i = 0; i < c.getCollectedErrorCount(); i++) errs.push(c.getCollectedError(i));
    c.free();

    if (expectError) {
        const got = errs.join(' | ').toLowerCase();
        if (errs.length > 0 && (!substr || got.includes(substr.toLowerCase()))) {
            pass++; console.log(`✓ ${name}`);
        } else {
            fail++; failures.push({name, errs}); console.log(`✗ ${name} — got: ${errs[0] || 'NO error'}`);
        }
    } else {
        if (errs.length === 0) { pass++; console.log(`✓ ${name}`); }
        else { fail++; failures.push({name, errs}); console.log(`✗ ${name} — got: ${errs[0]}`); }
    }
}

console.log('=== Conditional expression types ===');
check('mismatched ternary', true, 'void main() { int x = (1 > 0) ? 1 : "string"; }', 'matching');
check('matching ternary', false, 'void main() { int x = (1 > 0) ? 1 : 0; }');

console.log('\n=== If/while/for conditions ===');
check('if void condition', true, 'void f() {}\nvoid main() { if (f()) {} }', 'void');
check('while void condition', true, 'void f() {}\nvoid main() { while (f()) {} }', 'void');

console.log('\n=== String comparisons ===');
check('string == string', false, 'void main() { int x = "a" == "b"; }');
check('string != string', false, 'void main() { int x = "a" != "b"; }');
check('string < string', true, 'void main() { int x = "a" < "b"; }', 'comparison');

console.log('\n=== Object comparisons ===');
check('object == OBJECT_INVALID', false, 'void main() { object o = OBJECT_SELF; int x = o == OBJECT_INVALID; }');
check('object < object', true, 'void main() { object a = OBJECT_SELF; object b = OBJECT_SELF; int x = a < b; }', 'comparison');

console.log('\n=== Vector operations ===');
check('vector + vector', false, 'void main() { vector a = [1.0, 0.0, 0.0]; vector b = [0.0, 1.0, 0.0]; vector c = a + b; }');
check('vector * float', false, 'void main() { vector v = [1.0, 2.0, 3.0]; vector r = v * 2.0; }');
check('vector + int', true, 'void main() { vector v = [1.0, 2.0, 3.0]; vector r = v + 5; }', 'arithmetic');
check('vector.w (invalid)', true, 'void main() { vector v = [1.0, 2.0, 3.0]; float f = v.w; }', 'field');
check('vector.x + vector.y', false, 'void main() { vector v = [1.0, 2.0, 3.0]; float f = v.x + v.y; }');

console.log('\n=== Multiple statements per line ===');
check('two statements on one line', false, 'void main() { int x = 1; int y = 2; }');
check('three globals on one line', false, 'int a = 1; int b = 2; int c = 3;');

console.log('\n=== Line endings ===');
check('CRLF line endings', false, 'void main() {\r\n    int x = 1;\r\n}\r\n');

console.log('\n=== Unicode in strings ===');
check('unicode in string', false, 'void main() { string s = "héllo wörld"; }');

console.log('\n=== Include edge cases ===');
// C++ behavior: #include "foo.nss" with extension errors (ResMan looks for foo.nss.nss).
// We match that — users must write #include "foo" without extension.
{
    const c = new WasmCompiler();
    c.setRequireEntryPoint(false);
    c.setCollectAllErrors(true);
    c.addFile('lib', 'int helper() { return 1; }');
    c.addFile('t', '#include "lib.nss"\nvoid main() { int x = helper(); }');
    c.compile('t');
    const n = c.getCollectedErrorCount();
    c.free();
    if (n > 0) { pass++; console.log('✓ include with .nss extension errors (matches C++)'); }
    else { fail++; failures.push({name: 'include with .nss', errs: ['no error']}); console.log('✗ include with .nss — expected error to match C++'); }
}
check('empty include name', true, '#include ""\nvoid main() { }', 'not found');

console.log('\n=== Unreachable code ===');
check('code after return', false, 'int f() { return 1; int x = 5; return x; }');
// Note: most compilers don't error on this; just warn or accept.

console.log('\n=== Very long expressions ===');
check('deeply nested parens', false, 'void main() { int x = ((((((((((1)))))))))); }');
check('long arithmetic chain', false, 'void main() { int x = 1+2+3+4+5+6+7+8+9+10+11+12+13+14+15; }');

console.log('\n=== Negative literals ===');
check('negative int', false, 'void main() { int x = -5; }');
check('negative float', false, 'void main() { float f = -3.14; }');
check('negative in expression', false, 'void main() { int x = 10 + -5; }');
check('double negation', false, 'void main() { int x = -(-5); }');

console.log('\n=== Const struct field ===');
check('const struct decl', true, 'struct A { int x; };\nconst struct A X;', null);

console.log('\n=== Return mismatched in deeper paths ===');
check('return void in deeper path', true, 'int f() { if (1) { return; } return 5; }', null);

console.log('\n=== Empty switch case body ===');
check('case with empty body', false, 'void main() { switch (1) { case 1: break; } }');
check('fallthrough cases', false, 'void main() { switch (1) { case 1: case 2: case 3: break; } }');

console.log('\n=== Comment edge cases ===');
check('comment inside expression', false, 'void main() { int x = 1 /* comment */ + 2; }');
check('line comment then expression', false, 'void main() { int x = 1 // comment\n+ 2; }');

console.log(`\n=== ${pass + fail} tests: ${pass} passed, ${fail} failed ===`);
if (fail > 0) {
    failures.forEach(f => {
        console.log(`  ${f.name}: ${f.errs?.join(' | ') || 'NO error'}`);
    });
}
process.exit(fail > 0 ? 1 : 0);
