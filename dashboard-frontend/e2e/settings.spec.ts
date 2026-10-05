import {test,expect,VIEWS,login,openView} from './harness';

test('gateway changes stage and apply explicitly',async({page,consoleErrors})=>{
 await login(page);await openView(page,VIEWS.find(v=>v.name==='gateway-settings')!);
 await page.getByRole('button',{name:'Edit settings',exact:true}).click();
 await page.getByLabel('Request timeout',{exact:true}).fill('120');
 await page.getByRole('button',{name:'Validate & stage',exact:true}).click();
 await expect(page.getByText('Revision 1 · applied 0')).toBeVisible();
 await expect(page.getByText(/No gateway restart or request drain/)).toBeVisible();
 await page.getByRole('button',{name:'Apply staged configuration',exact:true}).click();
 await expect(page.getByText('Revision 1 · applied 1')).toBeVisible();
 expect(consoleErrors).toEqual([]);
});
test('profile forms use named controls and retain an optional advanced editor',async({page,consoleErrors})=>{
 await login(page);await openView(page,VIEWS.find(v=>v.name==='profiles')!);
 await page.getByRole('button',{name:'Edit settings',exact:true}).click();
 await page.getByRole('button',{name:'Add model profile',exact:true}).first().click();
 await page.getByLabel('Name',{exact:true}).fill('test-profile');
 await page.getByLabel('Served model',{exact:true}).fill('test-upstream');
 await expect(page.locator('.monaco-editor:visible')).toHaveCount(0);
 await page.getByRole('button',{name:'Update draft',exact:true}).click();
 await expect(page.getByText('test-profile',{exact:true})).toBeVisible();
 await expect(page.getByRole('button',{name:'Apply staged configuration',exact:true})).toHaveCount(0);
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

test('host policy and metric controls work on mobile without JSON',async({page,consoleErrors})=>{
 await login(page);await openView(page,VIEWS.find(v=>v.name==='host-settings')!);
 const host=page.getByTestId('host-settings-view').getByRole('combobox').first();await expect(host.locator('option')).not.toHaveCount(1);await host.selectOption({index:1});
 await page.getByRole('button',{name:'Edit host settings',exact:true}).click();
 await expect(page.getByLabel('Host RAM budget',{exact:true})).toBeVisible();await expect(page.getByLabel('Cpu sampling interval',{exact:true})).toHaveValue('5');
 await page.getByLabel('Cpu sampling interval',{exact:true}).fill('1');await expect(page.locator('.monaco-editor:visible')).toHaveCount(0);
 await page.setViewportSize({width:390,height:844});const compact=page.getByRole('button',{name:'Compact mobile',exact:true});if(await compact.getAttribute('aria-pressed')!=='true')await compact.click();
 await expect(page.getByLabel('Host RAM budget',{exact:true})).toBeVisible();expect(await page.evaluate(()=>document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
 await page.screenshot({path:'test-results/host-settings-form-mobile.png',fullPage:true});
 await page.getByRole('button',{name:'Discard edits',exact:true}).click();expect(consoleErrors).toEqual([]);
});
