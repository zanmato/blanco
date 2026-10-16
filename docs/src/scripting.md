# Scripting

Script tabs run JavaScript against the tab's connection. Use them for work
that is awkward in plain SQL, such as loops, data from one query feeding the
next, or reshaping rows before showing them.

<figure class="screenshot">
<img class="screenshot-light" src="images/script-latte.webp" alt="A script tab with JavaScript that queries the database and displays the result in the grid">
<img class="screenshot-dark" src="images/script-mocha.webp" alt="">
</figure>

Right click a connection or database and choose **New Script**. Run the script
with **Run Script**, and stop it with **Stop**.

## The `db` object

| Call | Does |
| --- | --- |
| `db.query(sql, params?)` | Runs a statement and returns its rows |
| `db.execute(sql, params?)` | Runs a statement and returns the number of rows it changed |
| `db.transaction(statements)` | Runs an array of statements as one transaction |
| `db.display(value)` | Shows an array of objects in the results grid |

`params` are bound to the statement's placeholders, written the way the
database expects them (see [Parameters](running-queries.md#parameters)).

`db.query` returns `rows` (one object per row, keyed by column name),
`columns`, `rowsAffected` and `executionTimeMs`.

```js
const customers = db.query("SELECT id, email FROM customers WHERE city = ?", ["Lund"]);
const summary = customers.rows.map((customer) => {
  const orders = db.query("SELECT count(*) AS total FROM orders WHERE customer_id = ?", [customer.id]);
  return { email: customer.email, orders: orders.rows[0].total };
});
db.display(summary);
```

`console.log`, `console.warn` and `console.error` write to the script tab's console
log, which the console log button shows and hides.

Calls block until the database answers, so there is no `await`. Scripts run in
a sandboxed QuickJS runtime on their own thread. Only `db` and `console` are
available, there is no file or network access. The script timeout in Settings stops scripts that run too long.

The editor completes the `db` calls and the fields of their results.
