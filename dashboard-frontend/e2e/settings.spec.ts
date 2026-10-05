import {test,expect,VIEWS,login,openView} from './harness';

test('gateway changes stage and apply explicitly',async({page,consoleErrors})=>{
 await login(page);await openView(page,VIEWS.find(v=>v.name==='gateway-settings')!);
 await page.getByRole('button',{name:'Edit settings',exact:true}).click();
 await page.getByLabel('request timeout secs',{exact:true}).fill('120');
 await page.getByRole('button',{name:'Validate & stage',exact:true}).click();
 await expect(page.getByText('Revision 1 · applied 0')).toBeVisible();
 await expect(page.getByText(/No gateway restart or request drain/)).toBeVisible();
 await page.getByRole('button',{name:'Apply staged configuration',exact:true}).click();
 await expect(page.getByText('Revision 1 · applied 1')).toBeVisible();
 expect(consoleErrors).toEqual([]);
});
test('profile JSON drafts use Monaco without submitting on add or format',async({page,consoleErrors})=>{
 await login(page);await openView(page,VIEWS.find(v=>v.name==='profiles')!);
 await page.getByRole('button',{name:'Edit settings',exact:true}).click();
 await page.getByRole('button',{name:'Add entry',exact:true}).first().click();
 await expect(page.locator('.monaco-editor').first()).toBeVisible();
 await page.getByRole('button',{name:'Format JSON',exact:true}).first().click();
 await expect(page.getByRole('button',{name:'Validate & stage',exact:true})).toBeVisible();
 await expect(page.getByRole('button',{name:'Apply staged configuration',exact:true})).toHaveCount(0);
 await page.getByRole('button',{name:'Cancel edit',exact:true}).click();
 expect(consoleErrors).toEqual([]);
});
test('all new management pages fit mobile and preserve masked secrets',async({page,consoleErrors})=>{
 await page.setViewportSize({width:390,height:844});await login(page);await page.getByRole('button',{name:'Compact mobile',exact:true}).click();
 for(const view of VIEWS.filter(v=>['gateway-settings','profiles','provider-settings','integrations','authentication','access-settings','observability','service','host-settings'].includes(v.name))){
  await page.getByLabel("Mobile dashboard page").selectOption(view.name);
  await expect(page.getByText(view.ready).first()).toBeVisible();
  const width=await page.evaluate(()=>({viewport:innerWidth,document:document.documentElement.scrollWidth}));
  expect(width.document,view.name).toBeLessThanOrEqual(width.viewport);
 }
 expect(consoleErrors).toEqual([]);
});
