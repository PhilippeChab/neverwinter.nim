const { WasmCompiler } = require('../pkg/nwscript_compiler.js');

let pass = 0, fail = 0;
const failures = [];

function expectError(name, src, substr) {
    const c = new WasmCompiler();
    c.setRequireEntryPoint(false);
    c.setCollectAllErrors(true);
    c.addFile('t', src);
    c.compile('t');
    const errs = [];
    for (let i = 0; i < c.getCollectedErrorCount(); i++) errs.push(c.getCollectedError(i));
    c.free();
    const got = errs.join(' | ').toLowerCase();
    if (errs.length > 0 && (!substr || got.includes(substr.toLowerCase()))) {
        pass++; console.log(`✓ ${name}`);
    } else {
        fail++; failures.push({name, errs, substr});
        console.log(`✗ ${name} — got: ${errs[0] || 'NO errors'}`);
    }
}
function expectOk(name, src) {
    const c = new WasmCompiler();
    c.setRequireEntryPoint(false);
    c.setCollectAllErrors(true);
    c.addFile('t', src);
    c.compile('t');
    const errs = [];
    for (let i = 0; i < c.getCollectedErrorCount(); i++) errs.push(c.getCollectedError(i));
    c.free();
    if (errs.length === 0) { pass++; console.log(`✓ ${name}`); }
    else { fail++; failures.push({name, errs}); console.log(`✗ ${name} — got: ${errs[0]}`); }
}

console.log('=== Const reassignment ===');
expectError('reassign const global', 'const int X = 5;\nvoid main() { X = 10; }', null);
expectError('reassign const local', 'void main() { const int X = 5; X = 10; }', null);

console.log('\n=== Empty / minimal ===');
expectOk('empty file', '');
expectOk('only comments', '// just a comment\n/* block */');
expectOk('empty function', 'void f() { }');
expectOk('empty switch', 'void main() { switch (1) {} }');

console.log('\n=== Comments ===');
expectOk('line comment at eof no newline', 'void main() {} // tail');
expectOk('block comment at eof no newline', 'void main() {} /* tail */');
expectOk('multi-line block comment', 'void main() {\n/* line1\n line2\n line3 */\n}');

console.log('\n=== String edge cases ===');
expectOk('empty string', 'void main() { string s = ""; }');
expectOk('string with escapes', 'void main() { string s = "\\n\\t\\\\\\""; }');
expectOk('string with quote escape', 'void main() { string s = "hello \\"world\\""; }');
expectError('unterminated string', 'void main() { string s = "unclosed', null);

console.log('\n=== Recursive struct ===');
expectError('struct contains self', 'struct A { struct A x; };', null);

console.log('\n=== Function call edge cases ===');
expectOk('recursive function', 'int fact(int n) { if (n <= 1) return 1; return n * fact(n - 1); }');
expectOk('call before decl', 'void main() { f(); }\nvoid f() { }');
expectError('object compare different types', 'void main() { if (OBJECT_SELF == 5) {} }', null);

console.log('\n=== Switch edge cases ===');
expectOk('case after default', 'void main() { switch(1) { default: break; case 1: break; } }');
expectOk('switch with no cases', 'void main() { switch(1) {} }');
expectOk('case fall-through', 'void main() { switch(1) { case 1: case 2: break; } }');

console.log('\n=== For loop edge cases ===');
expectOk('for with empty everything', 'void main() { for (;;) { break; } }');
expectOk('for with only condition', 'void main() { int i = 0; for (; i < 10; ) { i++; } }');
// C++ NWScript does NOT allow declarations in for-init; this rejects per C++ grammar.
expectError('for declaring loop var (C++ rejects)', 'void main() { for (int i = 0; i < 10; i++) {} }', 'bad start');

console.log('\n=== Operator edge cases ===');
expectOk('chained comparisons via &&', 'void main() { int x = 5; if (x > 0 && x < 10) {} }');
expectOk('parenthesized expression', 'void main() { int x = ((((1)))); }');

console.log('\n=== Function declaration edge cases ===');
expectOk('void in param list', 'void f();');
expectError('function param shadowing', 'void f(int x, int x) { }', null);

console.log('\n=== Include edge cases ===');
// A file that #includes ITSELF is silently deduplicated by the C++ compiler
// (the main file's own resref is registered before includes resolve), so it must
// compile cleanly with no error — not be flagged as recursive.
{
    const c = new WasmCompiler();
    c.setRequireEntryPoint(false);
    c.setCollectAllErrors(true);
    c.addFile('selfincl', '#include "selfincl"\nint X = 5;');
    c.compile('selfincl');
    const n = c.getCollectedErrorCount();
    const errs = [];
    for (let i = 0; i < n; i++) errs.push(c.getCollectedError(i));
    c.free();
    if (n === 0) { pass++; console.log('✓ self-include deduplicated (matches C++)'); }
    else { fail++; console.log('✗ self-include — spurious error:', errs[0]); failures.push({name: 'self-include'}); }
}

console.log(`\n=== ${pass + fail} tests: ${pass} passed, ${fail} failed ===`);
if (fail > 0) {
    console.log('\nFailures:');
    failures.forEach(f => {
        console.log(`  ${f.name}`);
        if (f.errs) f.errs.forEach(e => console.log(`    ${e}`));
    });
}
process.exit(fail > 0 ? 1 : 0);
