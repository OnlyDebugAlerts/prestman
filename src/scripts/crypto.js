// CryptoJS 4.x requests native cryptographic randomness through this API.
// No filesystem, Node.js modules or other host capabilities are exposed.
(() => {
    const randomWord = globalThis.__cryptoRandomU32;
    delete globalThis.__cryptoRandomU32;
    const integerArrays = [Int8Array, Uint8Array, Uint8ClampedArray, Int16Array,
        Uint16Array, Int32Array, Uint32Array, BigInt64Array, BigUint64Array];
    globalThis.crypto = Object.freeze({
        getRandomValues(array) {
            if (!integerArrays.some(type => array instanceof type)) {
                throw new TypeError('getRandomValues requires an integer TypedArray');
            }
            if (array.byteLength > 65536) throw new RangeError('getRandomValues exceeds the 65536 byte limit');
            const bytes = new Uint8Array(array.buffer, array.byteOffset, array.byteLength);
            for (let offset = 0; offset < bytes.length; offset += 4) {
                const word = randomWord();
                for (let byte = 0; byte < 4 && offset + byte < bytes.length; byte++) {
                    bytes[offset + byte] = word >>> (byte * 8);
                }
            }
            return array;
        },
    });
})();
