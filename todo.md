## TODO:

- [ ] Add abort trading logic. shut down after losing X amount
- [ ] Randomize strategy to allow automated strategy testing. Use previous reports so we dont repeat strategies.
- [ ] Dont allow reusing same strategy if earnings were negative. or at least warn about it.
- [ ] Add a column with an array of retrieved candles change over time. Ex: [0.2,-0,1,-2.0,-4.0,0.1,2.0,3.0]
- [ ] Move strategy to its own file and add hot-reload.
- [ ] Send alert if a token is above %X change. Notify volume as well.
- [ ] Start sending messages to a redpanda topic announcing tokens selected
- [ ] Create a portfolio topic in redpanda and send additions and removals
