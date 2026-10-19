import React from 'react';
import {createRoot} from 'react-dom/client';
import {Autocomplete} from '@povio/ui/tanstack/autocomplete';
const filler=Number(new URLSearchParams(location.search).get('fill')||0);
const items=[{id:'InsuranceProvider',label:'Insurance Provider'},{id:'Shipper',label:'Shipper'}];
function App(){const [value,setValue]=React.useState(null);return <div><div>{Array.from({length:filler},(_,i)=><div key={i} style={{height:20}}>filler</div>)}</div><Autocomplete label="Add Party" placeholder="Add Party" items={items} value={value} onChange={setValue} hideLabel hideDropdownIcon ignoreInputValueFiltering={false}/><div style={{height:900}}>end</div></div>}
createRoot(document.getElementById('root')).render(<App/>);
