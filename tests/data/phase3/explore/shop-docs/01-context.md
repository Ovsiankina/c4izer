# Context

The shop serves **customers** and *clerks*, with `HTTPS` between them.
See [the Structurizr site](https://structurizr.com) for the notation.

![](embed:context)

## Parts

1. The web app
2. The database
   - holds orders
   - holds stock

| Part | Technology |
|------|------------|
| Web | Rust |
| Database | PostgreSQL |

```
customer -> web "Orders through"
```

![Shop logo](logo.png)

![Remote badge](https://example.com/badge.png)

![](embed:no-such-view)
