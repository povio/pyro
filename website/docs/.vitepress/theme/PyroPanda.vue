<script setup lang="ts">
// The Pyro mascot: a red panda in front of a fire. Pure SVG + CSS animation.
// The tag on its collar carries Povio's "p" (the glyph from the povio.com wordmark).
const flame =
  'M200 400 C110 395 62 330 78 255 C86 222 104 200 118 186 C116 214 126 236 142 244 C134 200 150 150 186 108 C198 94 206 74 204 48 C238 82 252 126 246 168 C262 154 268 134 266 110 C300 142 330 200 326 262 C322 340 280 398 200 400Z';
const povioP =
  'M10.1738 5.44751C15.2203 5.44751 19.3309 9.75897 19.3477 15.0227C19.3475 20.3532 15.237 24.6809 10.1738 24.6809C7.46687 24.6642 4.94347 23.428 3.27246 21.3059V32.0002H1V15.0393C1 9.7589 5.11067 5.44757 10.1738 5.44751ZM10.1738 7.76977C6.36394 7.76984 3.27246 11.0289 3.27246 15.0393C3.27257 19.0998 6.36401 22.4084 10.1738 22.4084C13.967 22.4084 17.0584 19.0998 17.0752 15.0393C17.0752 11.0288 13.9838 7.76977 10.1738 7.76977Z';
const embers = [
  { x: 120, y: 300, r: 3, d: 0 },
  { x: 285, y: 280, r: 2.5, d: 0.9 },
  { x: 160, y: 250, r: 2, d: 1.7 },
  { x: 250, y: 230, r: 3, d: 2.4 },
  { x: 95, y: 340, r: 2, d: 3.1 },
  { x: 310, y: 330, r: 2.5, d: 1.3 },
  { x: 205, y: 150, r: 2, d: 2.0 },
];
</script>

<template>
  <div class="pyro-panda">
    <svg viewBox="0 0 400 420" role="img" aria-label="Pyro, a red panda in front of a fire">
      <defs>
        <radialGradient id="pp-glow" cx="50%" cy="62%" r="50%">
          <stop offset="0%" stop-color="#ff4d2e" stop-opacity="0.55" />
          <stop offset="55%" stop-color="#dc1f26" stop-opacity="0.18" />
          <stop offset="100%" stop-color="#dc1f26" stop-opacity="0" />
        </radialGradient>
        <linearGradient id="pp-fur" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stop-color="#e2582a" />
          <stop offset="100%" stop-color="#b8341a" />
        </linearGradient>
      </defs>

      <circle class="glow" cx="200" cy="260" r="200" fill="url(#pp-glow)" />

      <!-- Fire -->
      <g class="fire">
        <path class="f f1" :d="flame" fill="#8e0b10" />
        <path class="f f2" :d="flame" fill="#dc1f26" transform="translate(200 400) scale(0.8 0.8) translate(-200 -400)" />
        <path class="f f3" :d="flame" fill="#ff5a2a" transform="translate(200 400) scale(0.58 0.56) translate(-200 -400)" />
        <path class="f f4" :d="flame" fill="#ffa05a" transform="translate(200 400) scale(0.34 0.3) translate(-200 -400)" />
      </g>

      <!-- Tail -->
      <g class="tail">
        <path d="M262 334 C330 344 364 292 354 242 C346 202 364 174 386 170" fill="none" stroke="#c43c1e" stroke-width="44" stroke-linecap="round" />
        <path d="M262 334 C330 344 364 292 354 242 C346 202 364 174 386 170" fill="none" stroke="#6e1b0e" stroke-width="44" stroke-dasharray="15 21" stroke-dashoffset="-8" />
      </g>

      <g class="panda">
        <!-- Body -->
        <ellipse cx="200" cy="318" rx="80" ry="70" fill="url(#pp-fur)" />
        <ellipse cx="200" cy="338" rx="46" ry="48" fill="#4a1a12" />
        <rect x="156" y="320" width="32" height="64" rx="16" fill="#341109" />
        <rect x="212" y="320" width="32" height="64" rx="16" fill="#341109" />
        <ellipse cx="172" cy="384" rx="20" ry="10" fill="#2a0d07" />
        <ellipse cx="228" cy="384" rx="20" ry="10" fill="#2a0d07" />

        <!-- Head -->
        <g class="head">
          <path d="M138 172 L126 110 Q162 114 180 148 Z" fill="#fff4ec" />
          <path d="M142 162 L134 124 Q158 128 170 150 Z" fill="#341109" />
          <path d="M262 172 L274 110 Q238 114 220 148 Z" fill="#fff4ec" />
          <path d="M258 162 L266 124 Q242 128 230 150 Z" fill="#341109" />

          <ellipse cx="200" cy="202" rx="74" ry="62" fill="url(#pp-fur)" />
          <ellipse cx="148" cy="224" rx="24" ry="17" fill="#fff4ec" />
          <ellipse cx="252" cy="224" rx="24" ry="17" fill="#fff4ec" />
          <ellipse cx="200" cy="230" rx="36" ry="26" fill="#fff4ec" />
          <ellipse cx="176" cy="176" rx="11" ry="6" fill="#fff4ec" />
          <ellipse cx="224" cy="176" rx="11" ry="6" fill="#fff4ec" />
          <ellipse cx="174" cy="210" rx="8" ry="15" fill="#8f2a12" opacity="0.55" />
          <ellipse cx="226" cy="210" rx="8" ry="15" fill="#8f2a12" opacity="0.55" />

          <g class="eye"><circle cx="178" cy="198" r="8.5" fill="#1a0b08" /><circle cx="181" cy="195" r="2.6" fill="#fff" /></g>
          <g class="eye"><circle cx="222" cy="198" r="8.5" fill="#1a0b08" /><circle cx="225" cy="195" r="2.6" fill="#fff" /></g>

          <path d="M189 218 Q200 211 211 218 Q207 228 200 230 Q193 228 189 218Z" fill="#1a0b08" />
          <path d="M200 230 Q196 238 188 238 M200 230 Q204 238 212 238" fill="none" stroke="#1a0b08" stroke-width="2.5" stroke-linecap="round" />
        </g>

        <!-- Collar with Povio's p on the tag -->
        <path d="M146 256 Q200 282 254 256" fill="none" stroke="#1b1b1f" stroke-width="8" stroke-linecap="round" />
        <g class="tag">
          <circle cx="200" cy="284" r="12.5" fill="#f4f4f5" stroke="#1b1b1f" stroke-width="2" />
          <path :d="povioP" fill="#1b1b1f" transform="translate(194.4 273.7) scale(0.55)" />
        </g>
      </g>

      <!-- Embers -->
      <g class="embers">
        <circle v-for="(e, i) in embers" :key="i" :cx="e.x" :cy="e.y" :r="e.r" fill="#ff7a45" :style="{ animationDelay: `${e.d}s` }" />
      </g>
    </svg>
  </div>
</template>

<style scoped>
.pyro-panda {
  width: min(100%, 380px);
  margin: 0 auto;
}
svg {
  display: block;
  width: 100%;
  height: auto;
  overflow: visible;
}

.f {
  transform-box: fill-box;
  transform-origin: 50% 100%;
}
.f1 { animation: flicker 2.6s ease-in-out infinite; }
.f2 { animation: flicker 1.9s ease-in-out infinite reverse; }
.f3 { animation: flicker 1.4s ease-in-out infinite; }
.f4 { animation: flicker 1.1s ease-in-out infinite reverse; }
@keyframes flicker {
  0%, 100% { transform: scale(1, 1) skewX(0deg); }
  25% { transform: scale(1.03, 1.06) skewX(-2deg); }
  50% { transform: scale(0.98, 0.94) skewX(1.5deg); }
  75% { transform: scale(1.02, 1.04) skewX(-1deg); }
}

.glow {
  transform-box: fill-box;
  transform-origin: 50% 50%;
  animation: glow 3s ease-in-out infinite;
}
@keyframes glow {
  0%, 100% { opacity: 0.85; transform: scale(1); }
  50% { opacity: 1; transform: scale(1.04); }
}

.tail {
  transform-box: view-box;
  transform-origin: 262px 334px;
  animation: sway 3.2s ease-in-out infinite alternate;
}
@keyframes sway {
  from { transform: rotate(-6deg); }
  to { transform: rotate(5deg); }
}

.panda {
  animation: breathe 4s ease-in-out infinite;
}
@keyframes breathe {
  0%, 100% { transform: translateY(0); }
  50% { transform: translateY(-2px); }
}

.eye {
  transform-box: fill-box;
  transform-origin: 50% 50%;
  animation: blink 5s infinite;
}
@keyframes blink {
  0%, 92%, 100% { transform: scaleY(1); }
  95% { transform: scaleY(0.1); }
}

.tag {
  transform-box: fill-box;
  transform-origin: 50% 0%;
  animation: swing 4s ease-in-out infinite;
}
@keyframes swing {
  0%, 100% { transform: rotate(-6deg); }
  50% { transform: rotate(6deg); }
}

.embers circle {
  opacity: 0;
  animation: rise 3.6s ease-out infinite;
}
@keyframes rise {
  0% { opacity: 0; transform: translate(0, 0); }
  15% { opacity: 1; }
  100% { opacity: 0; transform: translate(10px, -150px); }
}

@media (prefers-reduced-motion: reduce) {
  .f, .glow, .tail, .panda, .eye, .tag, .embers circle {
    animation: none;
  }
  .embers circle { opacity: 0.7; }
}
</style>
