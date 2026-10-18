import {afterEach,beforeEach,expect,it,vi} from 'vitest';
import {cleanup,fireEvent,screen} from '@testing-library/react';
import {renderWithQuery,resetWorld} from '../../components/testHarness';
import {getConnection} from '../../api/connection';
import {HostArtifactsPanel} from './HostArtifactsPanel';
import {isArtifactInventory,type ArtifactInventory} from './artifactsModel';
const inventory:ArtifactInventory={sampled_at:'2026-10-18T13:00:00Z',presence_only:true,cache_configured:true,truncated:false,errors:[],images:[{id:`sha256:${'a'.repeat(64)}`,tags:['example:model'],digests:null,architecture:'arm64',os:'linux',size_bytes:1024**3}],snapshots:[{repository:'owner/model',revision:'b'.repeat(40),top_level_entries:3,top_level_logical_bytes:0,subfolders:['FL2VA'],truncated:false,unreadable_entries:0}]};
beforeEach(()=>resetWorld({mock:true}));afterEach(()=>{cleanup();vi.restoreAllMocks();});
it('inspects only on request and shows image metadata and presence-only checkpoints',async()=>{
 const inspect=vi.spyOn(getConnection().client,'hostArtifacts').mockResolvedValue(inventory);renderWithQuery(<HostArtifactsPanel endpointId="spark07"/>);expect(inspect).not.toHaveBeenCalled();fireEvent.click(screen.getByRole('button',{name:'Inspect host'}));expect(await screen.findByText('example:model')).toBeInTheDocument();expect(inspect).toHaveBeenCalledExactlyOnceWith('spark07');expect(screen.getByText('linux/arm64')).toBeInTheDocument();fireEvent.click(screen.getByRole('button',{name:'Checkpoints (1)'}));expect(screen.getByRole('link',{name:'owner/model'})).toHaveAttribute('href',`https://huggingface.co/owner/model/tree/${'b'.repeat(40)}`);expect(screen.getByText('FL2VA')).toBeInTheDocument();expect(screen.getByText(/Presence only/)).toBeInTheDocument();
});
it('surfaces an unsupported or offline host instead of silently showing an empty cache',async()=>{
 vi.spyOn(getConnection().client,'hostArtifacts').mockRejectedValue(new Error('Artifact inspection unavailable; update agent'));renderWithQuery(<HostArtifactsPanel endpointId="old-agent"/>);fireEvent.click(screen.getByRole('button',{name:'Inspect host'}));expect(await screen.findByRole('alert')).toHaveTextContent('update agent');
});
it('rejects unbounded or malformed artifact metadata',()=>{
 expect(isArtifactInventory(inventory)).toBe(true);expect(isArtifactInventory({...inventory,presence_only:false})).toBe(false);expect(isArtifactInventory({...inventory,images:[{...inventory.images[0],size_bytes:-1}]})).toBe(false);expect(isArtifactInventory({...inventory,snapshots:Array(129).fill(inventory.snapshots[0])})).toBe(false);
});
