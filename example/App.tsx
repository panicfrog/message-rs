/**
 * message-rs demo: proves JS, Swift and Kotlin share ONE Rust registry.
 * Runs the cross-language scenarios on launch and prints results (also to the console, so a
 * simulator run can be checked without tapping anything).
 */

import React, { useEffect, useState } from 'react';
import { SafeAreaView, ScrollView, StatusBar, StyleSheet, Text, View } from 'react-native';
import { runAll, type Result } from './src/scenarios';

function App(): React.JSX.Element {
  const [results, setResults] = useState<Result[]>([]);
  const [running, setRunning] = useState(true);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      const out = await runAll();
      if (cancelled) return;
      setResults(out);
      setRunning(false);
      // A single machine-readable line for automation / log scraping.
      const passed = out.filter((r) => r.ok).length;
      console.log(`HUB_DEMO_RESULT ${passed}/${out.length} ${JSON.stringify(out)}`);
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const allOk = !running && results.every((r) => r.ok);

  return (
    <SafeAreaView style={styles.root}>
      <StatusBar barStyle="dark-content" />
      <ScrollView contentContainerStyle={styles.body}>
        <Text style={styles.title}>message-rs data hub</Text>
        <Text style={styles.subtitle}>one Rust registry, three languages</Text>
        <Text style={[styles.banner, allOk ? styles.pass : running ? styles.wait : styles.fail]}>
          {running ? 'running…' : allOk ? 'ALL PASS' : 'FAILURES'}
        </Text>
        {results.map((r) => (
          <View key={r.name} style={styles.row}>
            <Text style={r.ok ? styles.ok : styles.bad}>{r.ok ? '✓' : '✗'}</Text>
            <View style={styles.rowBody}>
              <Text style={styles.name}>{r.name}</Text>
              <Text style={styles.detail}>{r.detail}</Text>
            </View>
          </View>
        ))}
      </ScrollView>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1, backgroundColor: '#fff' },
  body: { padding: 20 },
  title: { fontSize: 24, fontWeight: '700' },
  subtitle: { fontSize: 14, color: '#666', marginBottom: 16 },
  banner: { fontSize: 18, fontWeight: '700', padding: 12, borderRadius: 8, marginBottom: 16 },
  pass: { backgroundColor: '#e6f7e6', color: '#1a7f1a' },
  fail: { backgroundColor: '#fdeaea', color: '#b3261e' },
  wait: { backgroundColor: '#eee', color: '#555' },
  row: { flexDirection: 'row', paddingVertical: 8, borderBottomWidth: 1, borderBottomColor: '#eee' },
  rowBody: { flex: 1, marginLeft: 10 },
  ok: { color: '#1a7f1a', fontSize: 18 },
  bad: { color: '#b3261e', fontSize: 18 },
  name: { fontSize: 16, fontWeight: '600' },
  detail: { fontSize: 12, color: '#666', marginTop: 2 },
});

export default App;
