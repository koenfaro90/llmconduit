import {test,expect,login} from './harness';
for(const [width,height] of [[1600,1000],[1280,720],[390,844]])test(`Full request body tabs and exact clipboard at ${width}x${height}`,async({page,consoleErrors},info)=>{
 await page.context().grantPermissions(['clipboard-read','clipboard-write']);await page.setViewportSize({width,height});await login(page);await page.evaluate(()=>{location.hash='#/requests';});await page.getByLabel('Open request api_002',{exact:true}).locator('td').first().click();
 const inspector=page.getByTestId('flow-detail');await expect(inspector.getByRole('tab',{name:'Inbound request',exact:true})).toHaveAttribute('aria-selected','true');
 await expect(inspector.getByRole('button',{name:'Show raw',exact:true})).toBeVisible();
 await inspector.getByRole('button',{name:'Show raw',exact:true}).click();
 await expect(inspector.getByRole('button',{name:'Format JSON',exact:true})).toBeVisible();
 await inspector.getByRole('button',{name:'Format JSON',exact:true}).click();
 await inspector.locator('.view-lines .view-line').nth(2).waitFor();
 expect((await inspector.locator('.monaco-editor').boundingBox())!.height).toBeGreaterThan(150);
 for(const tab of ['Inbound request','Upstream request','Upstream response','Inbound response']){
  await inspector.getByRole('tab',{name:tab,exact:true}).click();await expect(inspector.getByRole('button',{name:'Copy body',exact:true})).toBeEnabled();await inspector.getByRole('button',{name:'Copy body',exact:true}).click();
  const copied=await page.evaluate(()=>navigator.clipboard.readText());
  if(tab==='Inbound request'){expect(copied.length).toBeGreaterThan(300_000);expect(copied.startsWith(' { "api_key" : "fixture-kept"')).toBe(true);expect(copied.endsWith('LAST-INBOUND-BYTE" }\r\n')).toBe(true);}
  else if(tab==='Upstream request'){expect(copied).toBe('{ "model" : "fixture", "tool_arguments":"\\u0061" }\r\n');}
  else expect(copied.endsWith('data: [DONE]\r\n\r\n')).toBe(true);
 }
 expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBe(true);await inspector.screenshot({path:info.outputPath('body-tabs.png')});expect(consoleErrors).toEqual([]);
});
