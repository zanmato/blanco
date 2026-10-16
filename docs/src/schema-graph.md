# Schema graph

Right click a database or schema and choose **View Schema Graph** to see its
tables and how their foreign keys connect them.

<figure class="screenshot">
<img class="screenshot-light" src="images/schema-graph-latte.webp" alt="The schema graph of the demo database with its tables and foreign key lines">
<img class="screenshot-dark" src="images/schema-graph-mocha.webp" alt="">
</figure>

Each table lists its columns and types, with primary and foreign keys marked.
Lines run from a foreign key column to the column it references. By default
only the lines of the selected table are drawn. Switch to show all of them.

Drag the background to move around and drag a table to move it.
