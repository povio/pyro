const target = document.createElement('div');
document.body.append(target);
target.getAnimations();
target.style.opacity = '0';
getComputedStyle(target).opacity;
target.style.transition = 'opacity 100s linear';
function move(value) {
  target.style.opacity = value;
  return target.getAnimations()[0];
}
const first = move('1');
first.currentTime = 25000;
const reverse = move('0');
const sample = Number(getComputedStyle(target).opacity);
reverse.currentTime = reverse.effect.getTiming().duration / 2;
const again = move('1');
again.currentTime = again.effect.getTiming().duration / 2;
const retarget = move('0.3');
const durations = [first, reverse, again, retarget].map(a => a.effect.getTiming().duration);
retarget.cancel();
target.remove();
function delayed(delay) {
  const element = document.createElement('div');
  element.style.opacity = '0';
  document.body.append(element);
  getComputedStyle(element).opacity;
  element.style.transition = `opacity 100s linear ${delay}s`;
  element.style.opacity = '1';
  const original = element.getAnimations()[0];
  original.currentTime = 25000 + delay * 1000;
  element.style.opacity = '0';
  const replacement = element.getAnimations()[0];
  const timing = replacement.effect.getTiming();
  replacement.cancel();
  element.remove();
  return [timing.duration, timing.delay];
}
console.log(JSON.stringify({sample, durations, negative: delayed(-10), positive: delayed(10)}));
