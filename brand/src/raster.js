const {chromium}=require('playwright');const fs=require('fs');const path=require('path');
const jobs=JSON.parse(process.argv[2]);
(async()=>{const b=await chromium.launch();const p=await b.newPage();
for(const [src,dst,w,h] of jobs){const s=fs.readFileSync(src,'utf8');await p.setViewportSize({width:w,height:h});
await p.setContent(`<html><body style="margin:0;background:transparent">${s.replace('<svg ','<svg width="'+w+'" height="'+h+'" style="display:block" ')}</body></html>`);
await p.screenshot({path:dst,omitBackground:true});}
await b.close()})();
