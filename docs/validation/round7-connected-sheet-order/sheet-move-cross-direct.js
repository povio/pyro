() => {
    const target=document.body.appendChild(document.createElement('div'));
    const left=document.body.appendChild(document.createElement('section'));
    const right=document.body.appendChild(document.createElement('section'));
    const add=(parent, value)=>{
        const sheet=document.createElement('style');
        sheet.textContent='div { --winner: '+value+' }';parent.append(sheet);return sheet;
    };
    const first=add(left,'first'),second=add(right,'second');
    const read=()=>getComputedStyle(target).getPropertyValue('--winner').trim();
    const result=[read()];
    right.append(first);result.push(read());
    left.append(first);result.push(read());
    right.insertBefore(first,second);result.push(read());
    return result;
}
