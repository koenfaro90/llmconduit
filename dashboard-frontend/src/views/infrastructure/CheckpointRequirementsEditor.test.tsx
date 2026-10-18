import {render,screen,fireEvent} from '@testing-library/react';
import {it,expect,vi} from 'vitest';
import {CheckpointRequirementsEditor} from './CheckpointRequirementsEditor';
it('offers a typed subfolder input and preserves the pinned requirement',()=>{
 const checkpoint={repository:'MiniMaxAI/MiniMax-H3',revision:'a'.repeat(40)};const onChange=vi.fn();
 render(<CheckpointRequirementsEditor value={[checkpoint]} onChange={onChange}/>);
 fireEvent.change(screen.getByLabelText('Checkpoint subfolder 1'),{target:{value:'FL2VA'}});
 expect(onChange).toHaveBeenCalledWith([{...checkpoint,subfolder:'FL2VA'}]);
});
