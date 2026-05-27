const { WasmCompiler } = require('./pkg/nwscript_compiler.js');
const c = new WasmCompiler();
c.setRequireEntryPoint(false);
c.setCollectAllErrors(true);

c.addFile('test', `
struct myStruct { int x; };
void foo(struct myStruct s) { }
struct myStruct bar() { struct myStruct s; return s; }
`);
let code = c.compile('test');
console.log('struct param test:', code === 0 ? 'PASS' : 'FAIL');
for (let i = 0; i < c.getCollectedErrorCount(); i++) {
    console.log('  Error:', c.getCollectedError(i));
}
c.free();
