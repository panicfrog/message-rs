const { getDefaultConfig, mergeConfig } = require('@react-native/metro-config');
const path = require('path');

/**
 * Metro configuration
 * https://reactnative.dev/docs/metro
 *
 * hub-turbo is installed as a `file:` symlink into the repository (../hub/turbo), so Metro
 * must watch the repo root and resolve modules from both node_modules trees — otherwise it
 * reports "Unable to resolve module hub-turbo" even though npm linked it.
 *
 * @type {import('@react-native/metro-config').MetroConfig}
 */
const repoRoot = path.resolve(__dirname, '..');

const config = {
  watchFolders: [repoRoot],
  resolver: {
    nodeModulesPaths: [
      path.resolve(__dirname, 'node_modules'),
      path.resolve(repoRoot, 'node_modules'),
    ],
    // the package's sources are TypeScript; Metro's default sourceExts already covers .ts
    disableHierarchicalLookup: false,
  },
};

module.exports = mergeConfig(getDefaultConfig(__dirname), config);
