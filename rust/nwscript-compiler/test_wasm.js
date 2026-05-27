const { WasmCompiler } = require('./pkg/nwscript_compiler.js');

const LANG_SPEC = `
int IntFn(int n);
string StringFn(string s);
void VoidFn();
`.trim();

let pass = 0;
let fail = 0;

function test(name, fn) {
    try {
        fn();
        pass++;
        console.log(`  ✓ ${name}`);
    } catch (e) {
        fail++;
        console.log(`  ✗ ${name}`);
        console.log(`      ${e.message}`);
    }
}

function assertEq(actual, expected, msg) {
    if (actual !== expected) {
        throw new Error(`${msg || 'assertEq'}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
    }
}

function assertContains(str, needle, msg) {
    if (!str.includes(needle)) {
        throw new Error(`${msg || 'assertContains'}: "${str}" does not contain "${needle}"`);
    }
}

// ====================================================

console.log('\nABI');
test('reports version 3', () => {
    const c = new WasmCompiler();
    assertEq(c.getABIVersion(), 3, 'ABI version');
    c.free();
});

console.log('\nHappy path');
test('valid void main compiles cleanly', () => {
    const c = new WasmCompiler();
    c.setLanguageSpec(LANG_SPEC);
    c.addFile('hello', 'void main() { int x = 1 + 2; }');
    const code = c.compile('hello');
    assertEq(code, 0, 'compile code');
    assertEq(c.getLastError(), '', 'no error');
    assertEq(c.getCollectedErrorCount(), 0, 'no collected errors');
    c.free();
});

test('valid script with #include', () => {
    const c = new WasmCompiler();
    c.setLanguageSpec(LANG_SPEC);
    c.addFile('lib', 'int helper(int n) { return n * 2; }');
    c.addFile('main', '#include "lib"\nvoid main() { int x = helper(3); }');
    const code = c.compile('main');
    assertEq(code, 0, 'compile code');
    c.free();
});

console.log('\nSingle-error mode');
test('undefined identifier reports name', () => {
    const c = new WasmCompiler();
    c.setLanguageSpec(LANG_SPEC);
    c.addFile('p', 'void main() { DoesNotExist(); }');
    const code = c.compile('p');
    if (code === 0) throw new Error('expected non-zero compile code');
    assertContains(c.getLastError(), 'DoesNotExist');
    c.free();
});

test('type mismatch', () => {
    const c = new WasmCompiler();
    c.setLanguageSpec(LANG_SPEC);
    c.addFile('p', 'void main() { string s = 42; }');
    const code = c.compile('p');
    if (code === 0) throw new Error('expected non-zero compile code');
    assertContains(c.getLastError(), 'Mismatched');
    c.free();
});

test('missing main', () => {
    const c = new WasmCompiler();
    c.setLanguageSpec(LANG_SPEC);
    c.addFile('p', 'int helper(int n) { return n; }');
    const code = c.compile('p');
    if (code === 0) throw new Error('expected non-zero compile code');
    c.free();
});

console.log('\nNo-entry-point flag');
test('include-only file compiles when flag is on', () => {
    const c = new WasmCompiler();
    c.setLanguageSpec(LANG_SPEC);
    c.setRequireEntryPoint(false);
    c.addFile('inc', 'int helper(int n) { return n; }');
    const code = c.compile('inc');
    assertEq(code, 0, 'compile code');
    c.free();
});

test('include-only file errors when flag is off', () => {
    const c = new WasmCompiler();
    c.setLanguageSpec(LANG_SPEC);
    c.addFile('inc', 'int helper(int n) { return n; }');
    const code = c.compile('inc');
    if (code === 0) throw new Error('expected non-zero compile code');
    c.free();
});

console.log('\nMulti-error mode');
test('three type errors in three functions', () => {
    const c = new WasmCompiler();
    c.setLanguageSpec(LANG_SPEC);
    c.setCollectAllErrors(true);
    c.setRequireEntryPoint(false);
    c.addFile('p', `
void main() { string s = 42; }
void other() { int i = "hello"; }
void third() { int j = "x"; }
`);
    c.compile('p');
    assertEq(c.getCollectedErrorCount(), 3, 'error count');
    for (let i = 0; i < 3; i++) {
        assertContains(c.getCollectedError(i), 'Mismatched');
    }
    c.free();
});

test('two undefined identifiers in two functions', () => {
    const c = new WasmCompiler();
    c.setLanguageSpec(LANG_SPEC);
    c.setCollectAllErrors(true);
    c.setRequireEntryPoint(false);
    c.addFile('p', `
void main() { DoesNotExistA(); }
void other() { DoesNotExistB(); }
`);
    c.compile('p');
    assertEq(c.getCollectedErrorCount(), 2, 'error count');
    assertContains(c.getCollectedError(0), 'DoesNotExistA');
    assertContains(c.getCollectedError(1), 'DoesNotExistB');
    c.free();
});

test('valid script in multi-error mode produces no errors', () => {
    const c = new WasmCompiler();
    c.setLanguageSpec(LANG_SPEC);
    c.setCollectAllErrors(true);
    c.addFile('p', 'void main() { int x = 1; }');
    const code = c.compile('p');
    assertEq(code, 0);
    assertEq(c.getCollectedErrorCount(), 0);
    c.free();
});

console.log('\nIncludes');
test('chained includes', () => {
    const c = new WasmCompiler();
    c.setLanguageSpec(LANG_SPEC);
    c.addFile('base', 'int base_fn(int n) { return n; }');
    c.addFile('mid', '#include "base"\nint mid_fn(int n) { return base_fn(n) + 1; }');
    c.addFile('main', '#include "mid"\nvoid main() { int x = mid_fn(5); }');
    const code = c.compile('main');
    assertEq(code, 0, 'compile code');
    c.free();
});

test('missing include file reports error', () => {
    const c = new WasmCompiler();
    c.setLanguageSpec(LANG_SPEC);
    c.addFile('main', '#include "nonexistent"\nvoid main() { }');
    const code = c.compile('main');
    if (code === 0) throw new Error('expected non-zero compile code');
    assertContains(c.getLastError(), 'not found');
    c.free();
});

test('state isolation: compile same file twice', () => {
    const c = new WasmCompiler();
    c.setLanguageSpec(LANG_SPEC);
    c.addFile('p', 'void main() { int x = 1; }');
    assertEq(c.compile('p'), 0);
    assertEq(c.compile('p'), 0);
    c.free();
});

test('engine function from lang spec resolves', () => {
    const c = new WasmCompiler();
    c.setLanguageSpec(LANG_SPEC);
    c.addFile('p', 'void main() { int x = IntFn(42); }');
    const code = c.compile('p');
    assertEq(code, 0, 'compile code');
    c.free();
});

// ====================================================

console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail > 0 ? 1 : 0);
