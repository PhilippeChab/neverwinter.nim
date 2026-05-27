const { WasmCompiler } = require('./pkg/nwscript_compiler.js');
const fs = require('fs');
const c = new WasmCompiler();
c.setLanguageSpec(fs.readFileSync('/home/philippechab/github/fru/nwscript.nss', 'utf-8'));
c.setRequireEntryPoint(false);
c.setCollectAllErrors(true);

c.addFile('test', `
struct commandStruct
{
    string sCommand;
    string sAlias;
    int nRole;
    string sScript;
    string sLabel;
};

json CommandToJson(struct commandStruct command)
{
    json jCommand = JsonObject();

    jCommand = JsonObjectSetString(jCommand, "command", command.sCommand);
    jCommand = JsonObjectSetString(jCommand, "alias", command.sAlias);
    jCommand = JsonObjectSetInt(jCommand, "role", command.nRole);
    jCommand = JsonObjectSetString(jCommand, "script", command.sScript);
    jCommand = JsonObjectSetString(jCommand, "label", command.sLabel);

    return jCommand;
}

struct commandStruct JsonToCommand(json jCommand)
{
    struct commandStruct command;

    command.sCommand = JsonObjectGetString(jCommand, "command");
    command.sAlias = JsonObjectGetString(jCommand, "alias");
    command.nRole = JsonObjectGetInt(jCommand, "role");
    command.sScript = JsonObjectGetString(jCommand, "script");
    command.sLabel = JsonObjectGetString(jCommand, "label");

    return command;
}
`);
let code = c.compile('test');
console.log('Result:', code === 0 ? 'PASS' : 'FAIL', '(' + c.getCollectedErrorCount() + ' errors)');
for (let i = 0; i < c.getCollectedErrorCount(); i++) {
    console.log('  Error:', c.getCollectedError(i));
}
c.free();
