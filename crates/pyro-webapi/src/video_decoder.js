(function (makeDecoder) {
  'use strict';
  return function (native) {
    function video({ dictionary, unsignedLong, descriptionView }) {
      const Frame = VideoFrame, ColorSpace = VideoColorSpace, U8 = Uint8Array;
      function readConfig(input) {
        const d = dictionary(input), c = Object.create(null);
        // Read and convert each Web IDL member once, in dictionary order.
        const codec = d.codec;
        if (codec === undefined) throw new TypeError('VideoDecoderConfig.codec is required.');
        c.codec = `${codec}`;
        for (const k of ['codedHeight', 'codedWidth']) {
          const value = d[k];
          if (value !== undefined) c[k] = unsignedLong(value);
        }
        const color = d.colorSpace;
        if (color !== undefined) c.colorSpace = new ColorSpace(color).toJSON();
        const description = d.description;
        if (description !== undefined) descriptionView(description);
        for (const k of ['displayAspectHeight', 'displayAspectWidth']) {
          const value = d[k];
          if (value !== undefined) c[k] = unsignedLong(value);
        }
        c.flip = Boolean(d.flip);
        const hardware = d.hardwareAcceleration;
        c.hardwareAcceleration = hardware === undefined ? 'no-preference' : `${hardware}`;
        if (!['no-preference', 'prefer-software', 'prefer-hardware'].includes(c.hardwareAcceleration))
          throw new TypeError('Invalid hardwareAcceleration.');
        c.optimizeForLatency = Boolean(d.optimizeForLatency);
        const rotation = d.rotation;
        c.rotation = rotation === undefined ? 0 : +rotation;
        if (!Number.isFinite(c.rotation)) throw new TypeError('Rotation must be finite.');
        if (!/[^\t\n\f\r ]/.test(c.codec)) throw new TypeError('Empty video codec.');
        for (const [width, height] of [['codedWidth', 'codedHeight'], ['displayAspectWidth', 'displayAspectHeight']]) {
          if ((c[width] === undefined) !== (c[height] === undefined) || c[width] === 0 || c[height] === 0)
            throw new TypeError('Dimensions must be a pair of positive integers.');
        }
        // A later dictionary getter can detach a description. Snapshot its
        // exact bytes only after all script-provided getters have finished.
        if (description !== undefined) c.description = new U8(descriptionView(description)).buffer;
        return c;
      }

      function output(record, c) {
        const { data, format, codedWidth, codedHeight, timestamp, duration } = record;
        let displayWidth = codedWidth, displayHeight = codedHeight;
        if (c.displayAspectWidth !== undefined) {
          const ratio = c.displayAspectWidth / c.displayAspectHeight;
          if (codedWidth / codedHeight < ratio) displayWidth = Math.round(codedHeight * ratio);
          else displayHeight = Math.round(codedWidth / ratio);
        }
        // VideoFrameInit takes the final display dimensions. Apply orientation
        // after deriving the unrotated size from the stream's display aspect.
        const turns = Math.floor(((((c.rotation % 360) + 360) % 360) + 45) / 90);
        if (turns % 2) [displayWidth, displayHeight] = [displayHeight, displayWidth];
        // The native conversion supplies packed sRGB pixels. VideoFrame's RGBA
        // defaults describe those bytes; YUV matrix metadata wouldn't.
        return new Frame(data, { format, codedWidth, codedHeight, timestamp,
          duration: duration ?? undefined, displayWidth, displayHeight, rotation: c.rotation, flip: c.flip });
      }

      return { readConfig, output, isKey: native.isKeyChunk };
    }
    return { VideoDecoder: makeDecoder(native, 'VideoDecoder', EncodedVideoChunk.prototype, video) };
  };
})
