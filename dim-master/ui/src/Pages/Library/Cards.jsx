import { useCallback, useEffect, useRef, useState } from "react";
import { useDispatch, useSelector } from "react-redux";
import { useParams } from "react-router";

import Card from "../../Components/Card/Index";
import Dropdown from "./Dropdown";
import useWebSocket from "../../hooks/ws";
import { scanLibrary } from "../../actions/library";
import RefreshIcon from "../../assets/Icons/Refresh";

import "./Cards.scss";

function Cards() {
  const dispatch = useDispatch();
  const params = useParams();
  const ws = useWebSocket();

  const user = useSelector((store) => store.user);
  const scanning = useSelector((store) => store.library.scanning);
  const isScanning = scanning.includes(parseInt(params.id));

  const [title, setTitle] = useState("");
  const [cards, setCards] = useState([]);

  /*
    held in this state temp until animation
    finishes swapping it.
  */
  const [newCards, setNewCards] = useState([]);

  const [fetched, setFetched] = useState(false);
  const [currentID, setCurrentID] = useState();
  const [show, setShow] = useState(false);

  const [throttleEventNewCardID, setThrottleEventNewCardID] = useState(false);

  const auth = useSelector((store) => store.auth);

  const cardList = useRef(null);

  const fetchCards = useCallback(
    async (reset = true) => {
      if (reset) {
        setNewCards([]);
      }

      try {
        const config = {
          headers: {
            authorization: auth.token,
          },
        };

        const res = await fetch(`/api/v1/library/${currentID}/media`, config);

        if (res.status !== 200) {
          return;
        }

        const payload = await res.json();

        setTitle(Object.keys(payload)[0]);
        setNewCards(Object.values(payload)[0]);
        setFetched(true);
      } catch (err) {}
    },
    [auth.token, currentID]
  );

  const handleWS = useCallback(
    (e) => {
      const { type, lib_id } = JSON.parse(e.data);

      if (type === "EventNewCard") {
        if (lib_id !== parseInt(params.id)) return;

        if (throttleEventNewCardID) {
          clearTimeout(throttleEventNewCardID);
          setThrottleEventNewCardID();
        }

        const id = setTimeout(() => {
          fetchCards(false);
        }, 500);

        setThrottleEventNewCardID(id);
      }
    },
    [fetchCards, params.id, throttleEventNewCardID]
  );

  useEffect(() => {
    if (!ws) return;

    ws.addEventListener("message", handleWS);
    return () => ws.removeEventListener("message", handleWS);
  }, [handleWS, ws]);

  useEffect(() => {
    if (!title) return;
    document.title = `Dim - ${title}`;
  }, [title]);

  useEffect(() => {
    if (currentID !== params.id) {
      setCurrentID(params.id);
      setShow(false);
    }
  }, [currentID, params.id]);

  useEffect(() => {
    if (!currentID) return;
    fetchCards();
  }, [currentID, fetchCards]);

  const handleTransitionEnd = useCallback((e) => {
    if (e.target !== cardList.current) return;
    if (e.propertyName !== "top") return;
  }, []);

  useEffect(() => {
    if (!cardList.current) return;

    const cards = cardList.current;

    cards.addEventListener("transitionend", handleTransitionEnd);

    return () =>
      cards.removeEventListener("transitionend", handleTransitionEnd);
  }, [handleTransitionEnd]);

  /*
    update cards list when hide animation
    ends and new cards have been fetched
  */
  useEffect(() => {
    if (show && fetched) {
      setCards(newCards);
    }
  }, [fetched, newCards, show]);

  /*
    when card list hides, cleanse cards
    set to ready to show new cards
  */
  const handleEnd = useCallback(async (e) => {
    if (e.animationName !== "hideCards") return;

    setCards([]);
    setShow(true);
  }, []);

  return (
    <div className="libraryCards" ref={cardList}>
      <div className="libraryHeader">
        <h2>{title.toLowerCase()}</h2>
        <div className="actions">
          {user.info.roles?.includes("owner") && (
            <button
              className={`rescanBtn${isScanning ? " scanning" : ""}`}
              onClick={() => { if (!isScanning) dispatch(scanLibrary(params.id)); }}
              disabled={isScanning}
              title={isScanning ? "Scanning..." : "Rescan library"}
            >
              <RefreshIcon />
            </button>
          )}
          <Dropdown />
        </div>
      </div>
      {show && fetched && newCards.length === 0 && (
        <p className="desc">No media has been found</p>
      )}
      <div
        className={`cards show-${show && fetched}`}
        onAnimationEnd={handleEnd}
      >
        {cards.map((card, i) => (
          <Card key={i} data={card} />
        ))}
      </div>
    </div>
  );
}

export default Cards;
