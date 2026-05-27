const { WasmCompiler } = require('./pkg/nwscript_compiler.js');

const c = new WasmCompiler();

// Minimal spec with just function declarations
const miniSpec = `
int Random(int nMaxInteger);
void PrintString(string sString);
json JsonObject();
effect EffectDamage(int nDamageAmount);
`;

c.setLanguageSpec(miniSpec);
c.setRequireEntryPoint(false);
c.setCollectAllErrors(true);

c.addFile('test', 'void main() { int x = Random(10); PrintString("hi"); json j = JsonObject(); }');
let code = c.compile('test');
console.log('Mini spec test:', code === 0 ? 'PASS' : 'FAIL');
for (let i = 0; i < c.getCollectedErrorCount(); i++) {
    console.log('  Error:', c.getCollectedError(i));
}

// Now try the actual spec but just the function part (after line 6390)
const fs = require('fs');
const fullSpec = fs.readFileSync('/home/philippechab/github/fru/nwscript.nss', 'utf-8');
const lines = fullSpec.split('\n');
console.log('\nTotal lines in nwscript.nss:', lines.length);
console.log('First #define:', lines.find(l => l.startsWith('#define')));
console.log('First function decl:', lines.find(l => l.match(/^\w+\s+\w+\s*\(/)));
console.log('Line 6393:', lines[6392]);

// Try loading just the function declarations
const funcLines = lines.filter(l => l.match(/^\w+\s+\w+\s*\(/) || l.match(/^\s+/));
const funcSpec = funcLines.join('\n');
console.log('\nFunction-only spec lines:', funcLines.length);

const c2 = new WasmCompiler();
c2.setLanguageSpec(funcSpec);
c2.setRequireEntryPoint(false);
c2.setCollectAllErrors(true);
c2.addFile('test2', 'void main() { int x = Random(10); }');
code = c2.compile('test2');
console.log('Func-only spec test:', code === 0 ? 'PASS' : 'FAIL');
for (let i = 0; i < c2.getCollectedErrorCount(); i++) {
    console.log('  Error:', c2.getCollectedError(i));
}
c2.free();
c.free();
