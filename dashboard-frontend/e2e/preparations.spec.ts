import {test,expect,login} from './harness';
for(const viewport of [{width:1600,height:1000},{width:390,height:844}]){
 test(`preparation controls fit ${viewport.width}px and stay read-only before selection`,async({page,consoleErrors},info)=>{
  await page.setViewportSize(viewport);await login(page);await page.evaluate(()=>{location.hash='#/deployment';});const panel=page.getByTestId('preparation-panel');await expect(panel).toBeVisible();await expect(panel.getByRole('button',{name:'Prepare',exact:true})).toBeDisabled();await expect(panel.getByLabel('Preparation recipe')).toBeVisible();await expect(panel.getByRole('alert')).toHaveCount(0);
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth+1)).toBe(true);await page.screenshot({path:info.outputPath('preparation.png'),fullPage:true});expect(consoleErrors).toEqual([]);
 });
}
