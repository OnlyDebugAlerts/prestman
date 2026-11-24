const data = pm.response.json();

pm.test('Response status is 200', () => {
  pm.response.to.have.status(200);
});
pm.test('Pre-request changed the HTTP method', () => {
  pm.expect(data.method).to.equal('POST');
});
pm.test('URL variable was substituted', () => {
  pm.expect(data.query.id).to.equal('123');
});
pm.test('Pre-request added a header', () => {
  pm.expect(data.headers['x-script-check']).to.equal('pre-ok');
});
pm.test('Pre-request changed the JSON body', () => {
  pm.expect(data.body.message).to.equal('hello from pre-script');
});

pm.environment.set('script_check', 'post-ok');
pm.test('Post-response saved an environment variable', () => {
  pm.expect(pm.environment.get('script_check')).to.equal('post-ok');
});
console.log('Post-response script executed', data);
