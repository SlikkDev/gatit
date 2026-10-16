// @actions/core writes GITHUB_OUTPUT using a delimited multiline value. Keep
// parsing strict: duplicate/malformed IDs cannot select a different artifact.
function artifactId(text) {
  const lines = text.replace(/\r\n/g, '\n').split('\n');
  const values = [];
  for (let i = 0; i < lines.length; i++) {
    if (lines[i].startsWith('artifact-id=')) values.push(lines[i].slice(12));
    else if (lines[i].startsWith('artifact-id<<')) {
      const delimiter = lines[i].slice(13);
      if (!delimiter || lines[i + 2] !== delimiter) throw new Error('Invalid artifact ID output');
      values.push(lines[++i]);
      i++;
    }
  }
  if (values.length !== 1 || !/^[1-9][0-9]*$/.test(values[0])) {
    throw new Error('GitHub did not confirm a unique artifact ID');
  }
  return values[0];
}
module.exports = { artifactId };
