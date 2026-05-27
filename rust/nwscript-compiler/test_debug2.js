const { WasmCompiler } = require('./pkg/nwscript_compiler.js');
const c = new WasmCompiler();
c.setRequireEntryPoint(false);
c.setCollectAllErrors(true);

// Test json return type with struct param
c.addFile('t1', 'json foo(struct myS s) { }');
c.compile('t1');
console.log('json + struct param:', c.getCollectedErrorCount() === 0 ? 'PASS' : 'FAIL');
for (let i = 0; i < c.getCollectedErrorCount(); i++) console.log(' ', c.getCollectedError(i));

// Test json return type with int param  
c.clearFiles();
c.addFile('t2', 'json foo(int n) { }');
c.compile('t2');
console.log('json + int param:', c.getCollectedErrorCount() === 0 ? 'PASS' : 'FAIL');
for (let i = 0; i < c.getCollectedErrorCount(); i++) console.log(' ', c.getCollectedError(i));

// Test effect return type
c.clearFiles();
c.addFile('t3', 'effect foo(int n) { }');
c.compile('t3');
console.log('effect return:', c.getCollectedErrorCount() === 0 ? 'PASS' : 'FAIL');
for (let i = 0; i < c.getCollectedErrorCount(); i++) console.log(' ', c.getCollectedError(i));

// Test with struct keyword explicitly
c.clearFiles();
c.addFile('t4', 'struct myS { int x; };\nint foo(struct myS s) { return s.x; }');
c.compile('t4');
console.log('int + struct param:', c.getCollectedErrorCount() === 0 ? 'PASS' : 'FAIL');
for (let i = 0; i < c.getCollectedErrorCount(); i++) console.log(' ', c.getCollectedError(i));

c.free();
