// WebDriver's Device Posture commands (https://w3c.github.io/device-posture/#automation), on the browser's emulated
// device posture as chromedriver drives them: CDP `Emulation.setDevicePostureOverride` / `clearDevicePostureOverride`.
// The posture is the value `navigator.devicePosture.type` and the `device-posture` media feature report (a desktop
// device is "continuous" until set), and a change fires `change` at `navigator.devicePosture`.

const POSTURES = ['continuous', 'folded'];

async function session(ctx, context) {
  return context ? ctx.sessionOf(await ctx.windowPage(context)) : ctx.session();
}

export const commands = {
  async set_device_posture(ctx, [posture, context]) {
    if (!POSTURES.includes(posture)) throw new Error(`invalid argument: ${posture} is not a device posture`);
    await (await session(ctx, context)).send('Emulation.setDevicePostureOverride', { posture: { type: posture } });
  },
  async clear_device_posture(ctx, [context]) {
    await (await session(ctx, context)).send('Emulation.clearDevicePostureOverride');
  },
};
