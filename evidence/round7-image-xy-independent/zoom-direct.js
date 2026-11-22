function(){return Array.from(document.images,i=>({id:i.id,x:i.x??null,y:i.y??null,rectY:i.getBoundingClientRect().y,height:i.getBoundingClientRect().height,naturalHeight:i.naturalHeight}))}
