() => {
const target = document.body.appendChild(document.createElement('div'));
        const a = document.body.appendChild(document.createElement('section'));
        const b = document.body.appendChild(document.createElement('section'));
        const add = (parent, value) => {
            const sheet = document.createElement('style');
            sheet.textContent = 'div { --winner: ' + value + ' }';
            parent.append(sheet); return sheet;
        };
        const first = add(a,'first'), second = add(a,'second');
        const read = () => getComputedStyle(target).getPropertyValue('--winner').trim();
        const result = [read()];
        a.append(first); result.push(read());
        b.append(second); result.push(read());
        document.body.append(a); result.push(read());
        return result;
}
