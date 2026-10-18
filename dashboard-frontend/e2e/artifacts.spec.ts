import {test,expect,login} from './harness';
for(const width of [1600,390]){
 test(`host artifact inspection is explicit and fits ${width}px`,async({page,consoleErrors},info)=>{
  await page.setViewportSize({width,height:844});await login(page);await page.evaluate(()=>{location.hash='#/hosts/vllm-a';});const panel=page.getByTestId('host-artifacts-panel');await expect(panel).toBeVisible();await expect(panel.getByText('example/omni:verified')).toHaveCount(0);await panel.getByRole('button',{name:'Inspect host'}).click();await expect(panel.getByText('example/omni:verified')).toBeVisible();await panel.getByRole('button',{name:'Checkpoints (1)'}).click();await expect(panel.getByRole('link',{name:'MiniMaxAI/MiniMax-H3'})).toBeVisible();expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBe(true);await panel.screenshot({path:info.outputPath('artifacts.png')});expect(consoleErrors).toEqual([]);
 });
}
