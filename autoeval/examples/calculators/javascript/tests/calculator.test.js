const { add } = require('../calculator');
test('zero', () => expect(add(0, 0)).toBe(0));
test('positive', () => expect(add(2, 3)).toBe(5));
test('negative', () => expect(add(-2, -3)).toBe(-5));
