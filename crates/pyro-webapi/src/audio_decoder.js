// Snapshot the four AudioDecoderConfig members in Web IDL dictionary order.
(function (makeDecoder) {
  'use strict';
  return function (native) {
    const U8 = Uint8Array;
    const AudioDecoder = makeDecoder(native, 'AudioDecoder', EncodedAudioChunk.prototype,
      ({ dictionary, unsignedLong, descriptionView }) => {
        function readConfig(input) {
          const d = dictionary(input);
          const codec = d.codec;
          if (codec === undefined) throw new TypeError('AudioDecoderConfig.codec is required.');
          const config = { codec: `${codec}` };
          const description = d.description;
          if (description !== undefined) descriptionView(description);
          for (const member of ['numberOfChannels', 'sampleRate']) {
            const value = d[member];
            if (value === undefined) throw new TypeError('AudioDecoderConfig.' + member + ' is required.');
            config[member] = unsignedLong(value);
          }
          if (!/[^\t\n\f\r ]/.test(config.codec) || !config.numberOfChannels || !config.sampleRate)
            throw new TypeError('Expected a codec and positive channel count and sample rate.');
          if (description !== undefined) config.description = new U8(descriptionView(description)).buffer;
          return config;
        }
        // Native output already owns its decoded sample planes as AudioData.
        return { readConfig, output: data => data };
      });
    return { AudioDecoder };
  };
})
