const fs = require("fs"), vm = require("vm");
const src = fs.readFileSync(process.argv[2], "utf8").replace(/^\uFEFF/, "");
const ctx = {}; vm.createContext(ctx);
vm.runInContext(src + "\n;this.__out = eddb;", ctx);
fs.writeFileSync(process.argv[3], JSON.stringify(ctx.__out));
console.log("ships", Object.keys(ctx.__out.ship).length, "modules", Object.keys(ctx.__out.module).length);
