import {test,expect,login} from './harness';
for(const width of [1600,390]){
 test(`API keys edit and rotate at ${width}px`,async({page,consoleErrors},info)=>{
  await page.setViewportSize({width,height:1000});await login(page);await page.evaluate(()=>{location.hash='#/account';});
  const row=page.getByTestId('key-row').filter({hasText:'laptop'});await expect(row).toBeVisible();
  await row.getByTestId('key-edit').click();const editor=page.getByTestId('key-editor');
  await editor.getByLabel('Key label',{exact:true}).fill('Edited workstation');await editor.getByLabel('Key owner',{exact:true}).selectOption('user_dev');
  await editor.getByLabel('New Allowed models',{exact:true}).fill('custom-alias');await editor.getByRole('button',{name:'Add',exact:true}).click();
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBe(true);
  await editor.screenshot({path:info.outputPath('key-editor.png')});await editor.getByRole('button',{name:'Save key changes'}).click();await expect(editor).toHaveCount(0);
  const saved=page.getByTestId('key-row').filter({hasText:'Edited workstation'});await expect(saved).toContainText('custom-alias');
  await saved.getByTestId('key-rotate').click();await page.getByRole('button',{name:'Generate replacement token'}).click();await expect(page.getByTestId('account-secret')).toContainText('llmc_mock_rotated');
  await page.getByRole('button',{name:'dismiss',exact:true}).click();await expect(page.getByTestId('account-secret')).toHaveCount(0);expect(consoleErrors).toEqual([]);
 });
}
