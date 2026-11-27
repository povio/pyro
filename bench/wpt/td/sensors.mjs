// WebDriver's virtual sensor commands (Generic Sensor "Automation": https://w3c.github.io/sensors/#automation), on the
// browser's virtual sensors as chromedriver drives them: CDP `Emulation.setSensorOverrideEnabled`,
// `setSensorOverrideReadings` and `getOverriddenSensorInformation`. A virtual sensor is the data source of the sensor
// APIs and of the device orientation and motion events; a reading goes through the same pipeline as a platform
// sensor's.

const XYZ = ['accelerometer', 'gravity', 'gyroscope', 'linear-acceleration', 'magnetometer'];
const ORIENTATION = ['absolute-orientation', 'relative-orientation'];
const TYPES = [...XYZ, ...ORIENTATION, 'ambient-light'];

const invalid = (m) => new Error(`invalid argument: ${m}`);

function type(t) {
  if (!TYPES.includes(t)) throw invalid(`${t} is not a virtual sensor type`);
  return t;
}

// The quaternion `[x, y, z, w]` of Euler angles in degrees (the Orientation Sensor "create a quaternion from Euler
// angles" algorithm: rotations about Z by alpha, X by beta, Y by gamma, intrinsic).
function quaternion({ alpha, beta, gamma }) {
  const half = (deg) => { const r = (deg * Math.PI) / 360; return [Math.sin(r), Math.cos(r)]; };
  const [sz, cz] = half(alpha), [sx, cx] = half(beta), [sy, cy] = half(gamma);
  return { x: cy * cz * sx - cx * sz * sy, y: cz * cx * sy + cy * sz * sx, z: cy * cx * sz + cz * sx * sy, w: cz * cx * cy - sz * sx * sy };
}

// WebDriver "parse a reading" of each virtual sensor type: the members must be finite numbers.
function reading(t, r) {
  const numbers = (names) => {
    if (r === null || typeof r !== 'object') throw invalid('the reading must be an object');
    for (const name of names) if (typeof r[name] !== 'number' || !Number.isFinite(r[name])) throw invalid(`${name} must be a finite number`);
  };
  if (XYZ.includes(t)) { numbers(['x', 'y', 'z']); return { xyz: { x: r.x, y: r.y, z: r.z } }; }
  if (ORIENTATION.includes(t)) { numbers(['alpha', 'beta', 'gamma']); return { quaternion: quaternion(r) }; }
  numbers(['illuminance']);
  return { single: { value: r.illuminance } };
}

async function session(ctx, context) {
  return context ? ctx.sessionOf(await ctx.windowPage(context)) : ctx.session();
}

export const commands = {
  async create_virtual_sensor(ctx, [sensorType, params, context]) {
    const t = type(sensorType);
    const p = params ?? {};
    const metadata = {};
    if (p.connected !== undefined) metadata.available = !!p.connected;
    if (p.minSamplingFrequency !== undefined) metadata.minimumFrequency = p.minSamplingFrequency;
    if (p.maxSamplingFrequency !== undefined) metadata.maximumFrequency = p.maxSamplingFrequency;
    await (await session(ctx, context)).send('Emulation.setSensorOverrideEnabled', { enabled: true, type: t, metadata });
  },
  async update_virtual_sensor(ctx, [sensorType, r, context]) {
    const t = type(sensorType);
    await (await session(ctx, context)).send('Emulation.setSensorOverrideReadings', { type: t, reading: reading(t, r) });
  },
  async remove_virtual_sensor(ctx, [sensorType, context]) {
    const t = type(sensorType);
    await (await session(ctx, context)).send('Emulation.setSensorOverrideEnabled', { enabled: false, type: t });
  },
  async get_virtual_sensor_information(ctx, [sensorType, context]) {
    const t = type(sensorType);
    const { requestedSamplingFrequency } = await (await session(ctx, context)).send('Emulation.getOverriddenSensorInformation', { type: t });
    return { requestedSamplingFrequency };
  },
};
