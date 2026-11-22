// The list and track wrappers share one per-realm set of private states.
(function(makeCues, parseFile) {
  'use strict';
  return function(native) {
    const cueAPI = makeCues(native.decodeCueText, native.foreignState);
    const {cueOwner, setCueOwner, cueTiming, parsedCue, parsedRegion} = cueAPI;
    const token = Symbol(), tracks = new WeakMap(), lists = new WeakMap(), cueLists = new WeakMap();
    const elementTracks = new WeakMap(), mediaLists = new WeakMap(), eventTracks = new WeakMap();
    const kinds = ['subtitles','captions','descriptions','chapters','metadata'];
    const E = Event, DE = DOMException;
    const attr = Function.prototype.call.bind(Element.prototype.getAttribute);
    const listen = Function.prototype.call.bind(EventTarget.prototype.addEventListener);
    const unlisten = Function.prototype.call.bind(EventTarget.prototype.removeEventListener);
    const mediaTime = Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, 'currentTime').get;
    const slot = (map, o) => {
      const s = map.get(o) ?? (map === tracks ? native.foreignState('track',o) : undefined);
      if (!s) throw new TypeError('Illegal invocation');
      return s;
    };
    const requireToken = x => { if (x !== token) throw new TypeError('Illegal constructor'); };
    const required = n => { if (!n) throw new TypeError('Missing argument'); };
    const string = v => `${v}`;
    function queue(target, event) { native.postTask(() => native.fire(target, event)); }
    class TrackEvent extends E {
      constructor(type, init = {}) {
        required(arguments.length);
        super(type, init);
        const track = init?.track ?? null;
        if (track !== null && !tracks.has(track) && !native.foreignState('track',track)) throw new TypeError('Expected a track');
        eventTracks.set(this, track);
      }
      get track() {
        if (!eventTracks.has(this)) throw new TypeError('Illegal invocation');
        return eventTracks.get(this);
      }
    }
    function handler(proto, type) {
      const handlers = new WeakMap();
      Object.defineProperty(proto, 'on' + type, {
        enumerable: true, configurable: true,
        get() { return handlers.get(this)?.value ?? null; },
        set(value) {
          const old = handlers.get(this);
          if (!value || !['object','function'].includes(typeof value)) {
            if (old) unlisten(this, type, old.listener);
            handlers.delete(this);
          } else if (old) old.value = value;
          else {
            const h = {value, listener(event) {
              if (typeof h.value === 'function') h.value.call(this, event);
            }};
            handlers.set(this, h);
            listen(this, type, h.listener);
          }
        }
      });
    }
    function resetCues(s) {
      for (const c of s.cues) setCueOwner(c, null);
      s.cues = [];
      s.active = [];
      s.previousCues.clear();
    }
    function updateSource(s) {
      if (!s.element) return;
      const src = native.sourceURL(s.element);
      if (src !== s.src) {
        s.src = src;
        resetCues(s);
      }
    }
    function sorted(s) {
      updateSource(s);
      return s.cues.slice().sort((a,b) => {
        const x = cueTiming(a), y = cueTiming(b);
        return x[0] - y[0] || y[1] - x[1];
      });
    }
    function active(s, time) {
      return sorted(s).filter(c => {
        const [start,end] = cueTiming(c);
        return start <= time && time < end;
      });
    }
    function cueValues(list) {
      const {s, onlyActive} = slot(cueLists, list);
      return onlyActive ? (s.mode !== 'disabled' && s.media ? active(s, mediaTime.call(s.media)) : []) : sorted(s);
    }
    const index = key => typeof key === 'string' && String(key >>> 0) === key && key !== '4294967295';
    class TextTrackCueList {
      constructor(secret, s, onlyActive) {
        requireToken(secret);
        const list = new Proxy(this, {
          get(t,k,r) { return index(k) ? cueValues(list)[k] : Reflect.get(t,k,r); },
          has(t,k) { return index(k) ? Number(k) < cueValues(list).length : Reflect.has(t,k); },
          ownKeys(t) { return [...cueValues(list).keys()].map(String).concat(Reflect.ownKeys(t)); },
          getOwnPropertyDescriptor(t,k) {
            if (!index(k)) return Reflect.getOwnPropertyDescriptor(t,k);
            const value = cueValues(list)[k];
            return value && {value, writable:false, enumerable:true, configurable:true};
          },
          defineProperty(t,k,d) { return !index(k) && Reflect.defineProperty(t,k,d); },
          deleteProperty(t,k) { return index(k) ? !(k in list) : Reflect.deleteProperty(t,k); },
          preventExtensions() { return false; }
        });
        cueLists.set(list, {s, onlyActive});
        return list;
      }
      get length() { return cueValues(this).length; }
      getCueById(id) {
        required(arguments.length); id = string(id);
        return id ? cueValues(this).find(c => cueTiming(c)[2] === id) ?? null : null;
      }
      [Symbol.iterator]() { return Array.prototype.values.call(this); }
    }
    class TextTrack extends EventTarget {
      constructor(secret, s) {
        requireToken(secret); super(); tracks.set(this,s);
        s.track = this; s.cues = []; s.active = []; s.previousCues = new Set();
        s.all = new TextTrackCueList(token,s,false);
        s.current = new TextTrackCueList(token,s,true);
      }
      get kind() {
        const s = slot(tracks,this);
        if (!s.element) return s.kind;
        const raw = attr(s.element,'kind');
        if (raw === null) return 'subtitles';
        const k = raw.toLowerCase();
        return kinds.includes(k) ? k : 'metadata';
      }
      get label() { const s=slot(tracks,this); return s.element ? attr(s.element,'label') ?? '' : s.label; }
      get language() { const s=slot(tracks,this); return s.element ? attr(s.element,'srclang') ?? '' : s.language; }
      get id() { const s=slot(tracks,this); return s.element ? attr(s.element,'id') ?? '' : ''; }
      get inBandMetadataTrackDispatchType() { slot(tracks,this); return ''; }
      get mode() { return slot(tracks,this).mode; }
      set mode(v) {
        const s=slot(tracks,this); v=string(v);
        if (!['disabled','showing','hidden'].includes(v)) return;
        s.explicit = true;
        if (s.mode === v) return;
        s.mode = v;
        if (s.element) native.check(s.element);
        if (s.media) {
          const list=mediaLists.get(s.media);
          queue(list, new E('change'));
          updatePlayback(lists.get(list), false);
        }
      }
      get cues() { const s=slot(tracks,this); updateSource(s); return s.mode === 'disabled' ? null : s.all; }
      get activeCues() { const s=slot(tracks,this); return s.mode === 'disabled' ? null : s.current; }
      addCue(cue) {
        const s=slot(tracks,this); required(arguments.length); updateSource(s);
        const owner=cueOwner(cue);
        if (owner === this) return;
        if (owner) removeFrom(slot(tracks,owner),cue);
        setCueOwner(cue,this);
        s.cues.push(cue);
      }
      removeCue(cue) {
        const s=slot(tracks,this); required(arguments.length); updateSource(s);
        if (cueOwner(cue) !== this) throw new DE('Cue is not in this track','NotFoundError');
        removeFrom(s,cue);
      }
    }
    function removeFrom(s,cue) {
      s.cues.splice(s.cues.indexOf(cue),1);
      setCueOwner(cue,null);
    }
    function elementTrack(el) {
      let track=elementTracks.get(el);
      if (!track) {
        track=new TextTrack(token,{element:el,media:null,mode:'disabled',src:native.sourceURL(el)});
        elementTracks.set(el,track);
      }
      return track;
    }
    function updatePlayback(l, normal) {
      const now=mediaTime.call(l.media), prev=l.lastTime;
      l.lastTime=now;
      const events=[], affected=[];
      let pause=false;
      for (let trackIndex=0;trackIndex<l.items.length;trackIndex++) {
        const track=l.items[trackIndex], s=tracks.get(track);
        const ordered=sorted(s), next=s.mode==='disabled' ? [] : active(s,now);
        const previous=s.active;
        const count=events.length;
        for (const cue of new Set([...ordered,...previous])) {
          const [start,end,,pauseOnExit]=cueTiming(cue);
          const was=previous.includes(cue), is=next.includes(cue);
          const missed=normal && s.mode!=='disabled' && now>prev && start>=prev && start<=now &&
            end<=now && !was && !is && s.previousCues.has(cue) && ordered.includes(cue);
          const cueIndex=ordered.indexOf(cue);
          if ((!was && is) || missed) events.push({time:start,trackIndex,cueIndex,cue,type:'enter'});
          if ((was && !is) || missed) {
            events.push({time:Math.max(start,end),trackIndex,cueIndex,cue,type:'exit'});
            if (normal && pauseOnExit) pause=true;
          }
        }
        s.active=next;
        s.previousCues=new Set(ordered);
        if (events.length!==count) affected.push(s);
      }
      events.sort((a,b)=>a.time-b.time || a.trackIndex-b.trackIndex || a.cueIndex-b.cueIndex ||
        (a.type===b.type ? 0 : a.type==='enter' ? -1 : 1));
      for (const {cue,type} of events) queue(cue,new E(type));
      for (const s of affected) {
        queue(s.track,new E('cuechange'));
        if (s.element) queue(s.element,new E('cuechange'));
      }
      return pause;
    }
    class TextTrackList extends EventTarget {
      constructor(secret,l) { requireToken(secret); super(); lists.set(this,l); }
      get length() { const l=slot(lists,this); syncMedia(l.media); return l.items.length; }
      getTrackById(id) {
        const l=slot(lists,this); required(arguments.length); id=string(id); syncMedia(l.media);
        return l.items.find(t=>t.id===id) ?? null;
      }
      [Symbol.iterator]() { return Array.prototype.values.call(this); }
    }
    function syncMedia(media) {
      let list=mediaLists.get(media);
      if (!list) {
        const l={media, items:[], script:[], lastTime:mediaTime.call(media)};
        list=new TextTrackList(token,l); mediaLists.set(media,list);
      }
      const l=lists.get(list);
      const elements=native.children(media).map(elementTrack);
      const items=elements.concat(l.script), old=l.items;
      l.items=items;
      for (const t of old) if (!items.includes(t)) {
        const s=tracks.get(t);
        if (s.media===media) {s.media=null; s.active=[];}
        queue(list,new TrackEvent('removetrack',{track:t}));
      }
      for (const t of items) if (!old.includes(t)) {
        tracks.get(t).media=media;
        queue(list,new TrackEvent('addtrack',{track:t}));
      }
      for (let i=0;i<items.length;i++) if (old[i]!==items[i])
        Object.defineProperty(list,i,{enumerable:true,configurable:true,get:()=>{syncMedia(media);return l.items[i];}});
      for (let i=items.length;i<old.length;i++) delete list[i];
      // Select the first default in each kind group when the user has made no selection.
      const groups=new Set();
      for (const t of elements) {
        const s=tracks.get(t), group=['subtitles','captions'].includes(t.kind)?'captions':t.kind;
        if (s.mode!=='disabled') groups.add(group);
        else if (!s.explicit && !groups.has(group) && attr(s.element,'default')!==null) {
          s.mode=group==='metadata'?'hidden':'showing';
          groups.add(group);
        }
      }
      return list;
    }
    function addTextTrack(media,kind,label,language) {
      kind=string(kind);
      if (!kinds.includes(kind)) throw new TypeError('Invalid text track kind');
      label=label===undefined?'':string(label); language=language===undefined?'':string(language);
      const l=lists.get(syncMedia(media));
      const track=new TextTrack(token,{kind,label,language,media,mode:'hidden'});
      l.script.push(track); syncMedia(media);
      return track;
    }
    for (const type of ['addtrack','removetrack','change']) handler(TextTrackList.prototype,type);
    handler(TextTrack.prototype,'cuechange');
    for (const C of [TextTrack,TextTrackList,TextTrackCueList,TrackEvent]) {
      for (const key of Object.getOwnPropertyNames(C.prototype)) if (key!=='constructor')
        Object.defineProperty(C.prototype,key,{enumerable:true});
      Object.defineProperty(C.prototype,Symbol.toStringTag,{value:C.name,configurable:true});
    }
    return {
      ...cueAPI,TextTrack,TextTrackList,TextTrackCueList,TrackEvent,syncMedia,addTextTrack,elementTrack,
      localState(kind,obj) {return kind==='track' ? tracks.get(obj) : cueAPI.localState(kind,obj);},
      advancePlayback(media,normal) {
        const list=mediaLists.get(media);
        return !!list && updatePlayback(lists.get(list),normal);
      },
      nextPause(media,position) {
        const list=mediaLists.get(media);
        let earliest=Infinity;
        if (list) for (const track of lists.get(list).items) {
          const s=tracks.get(track);
          if (s.mode==='disabled') continue;
          for (const cue of s.cues) {
            const [start,end,,pause]=cueTiming(cue), boundary=Math.max(start,end);
            if (pause && boundary>position && boundary<earliest) earliest=boundary;
          }
        }
        return earliest;
      },
      resetResourceCues(el) {
        const s=tracks.get(elementTrack(el));
        s.src=native.sourceURL(el);
        resetCues(s);
      },
      trackMode(el) {return tracks.get(elementTrack(el)).mode;},
      clearCues(el) {const s=tracks.get(elementTrack(el)); updateSource(s);},
      loadText(el,text) {
        const file=parseFile(text);
        if (!file) return false;
        const s=tracks.get(elementTrack(el)); updateSource(s);
        const regions=file.regions.map(r=>parsedRegion(r));
        for (const c of file.cues) {
          const cue=parsedCue(c,regions); setCueOwner(cue,s.track); s.cues.push(cue);
        }
        return true;
      }
    };
  };
})
