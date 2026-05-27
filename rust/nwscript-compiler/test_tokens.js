const { WasmCompiler } = require('./pkg/nwscript_compiler.js');
const c = new WasmCompiler();
c.setRequireEntryPoint(false);
c.setCollectAllErrors(true);

// Simplest case - json return type, struct parameter
c.addFile('t1', 'json foo(struct myS cmd) { }');
c.compile('t1');
console.log('t1 json foo(struct myS cmd):', c.getCollectedErrorCount() === 0 ? 'PASS' : 'FAIL');
for (let i = 0; i < c.getCollectedErrorCount(); i++) console.log(' ', c.getCollectedError(i));

// Even simpler - does struct param work at all?
c.clearFiles();
c.addFile('t2', 'void foo(struct myS cmd) { }');
c.compile('t2');
console.log('t2 void foo(struct myS cmd):', c.getCollectedErrorCount() === 0 ? 'PASS' : 'FAIL');
for (let i = 0; i < c.getCollectedErrorCount(); i++) console.log(' ', c.getCollectedError(i));

// Does struct DEFINITION + struct param work?
c.clearFiles();
c.addFile('t3', 'struct myS { int x; };\nvoid foo(struct myS cmd) { }');
c.compile('t3');
console.log('t3 struct def + param:', c.getCollectedErrorCount() === 0 ? 'PASS' : 'FAIL');
for (let i = 0; i < c.getCollectedErrorCount(); i++) console.log(' ', c.getCollectedError(i));

// Multi-field struct def + json return + struct param
c.clearFiles();
c.addFile('t4', 'struct myS { string a; int b; };\njson foo(struct myS cmd) { }');
c.compile('t4');
console.log('t4 multi-field struct + json return + struct param:', c.getCollectedErrorCount() === 0 ? 'PASS' : 'FAIL');
for (let i = 0; i < c.getCollectedErrorCount(); i++) console.log(' ', c.getCollectedError(i));

c.free();
